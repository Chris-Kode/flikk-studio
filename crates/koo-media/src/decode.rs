//! Frame and audio decode: single frames (cached), streaming playback and PCM.
//!
//! Video comes out of FFmpeg as a stream of PAM images, whose headers carry
//! the frame size, so rotation and pixel aspect never have to be predicted.

use crate::{FfmpegMedia, err, is_still_image, proxy, secs_arg, stderr_tail};
use koo_core::model::{MediaItem, MediaKind};
use koo_core::services::{Frame, FrameStream, PcmStream, ServiceError, ServiceResult};
use koo_core::time::{Rate, Time};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};

// --------------------------------------------------------------- frame cache

const FRAME_CACHE_BYTES: usize = 96 << 20;

type FrameKey = (PathBuf, i64, u32);

/// Least recently used decoded frames, bounded by total pixel bytes.
#[derive(Default)]
pub(crate) struct FrameCache {
    entries: VecDeque<(FrameKey, Frame)>,
    bytes: usize,
}

impl FrameCache {
    fn get(&mut self, key: &FrameKey) -> Option<Frame> {
        let i = self.entries.iter().position(|(k, _)| k == key)?;
        let entry = self.entries.remove(i)?;
        let frame = entry.1.clone();
        self.entries.push_back(entry);
        Some(frame)
    }

    fn put(&mut self, key: FrameKey, frame: Frame) {
        if let Some(i) = self.entries.iter().position(|(k, _)| *k == key)
            && let Some((_, old)) = self.entries.remove(i)
        {
            self.bytes -= old.rgba.len();
        }
        self.bytes += frame.rgba.len();
        self.entries.push_back((key, frame));
        while self.bytes > FRAME_CACHE_BYTES && self.entries.len() > 1 {
            if let Some((_, old)) = self.entries.pop_front() {
                self.bytes -= old.rgba.len();
            }
        }
    }
}

// -------------------------------------------------------------- single frame

pub(crate) fn frame(
    media: &FfmpegMedia,
    item: &MediaItem,
    t: Time,
    max_width: u32,
) -> ServiceResult<Frame> {
    if item.kind == MediaKind::Image {
        return still(media, &item.path, None, max_width);
    }
    let info = media.info(item)?;
    let rate = info.video.as_ref().map(|v| v.rate).unwrap_or_default();
    // Past the end there is no frame; show the last one instead.
    let mut t = t.max(Time::ZERO);
    if info.duration > Time::ZERO {
        t = t.min((info.duration - rate.frame_duration()).max(Time::ZERO));
    }
    still(media, &video_source(item), Some((t, rate)), max_width)
}

/// The frame of the file at `path` that is showing at a time, on a file of
/// the given rate; cached. `None` decodes the first frame.
pub(crate) fn still(
    media: &FfmpegMedia,
    path: &Path,
    at: Option<(Time, Rate)>,
    max_width: u32,
) -> ServiceResult<Frame> {
    let at = at.filter(|_| !is_still_image(path));
    let start = at.map(|(t, rate)| t.floor_to_frame(rate));
    let key = (path.to_path_buf(), start.map_or(-1, |t| t.0), max_width);
    if let Some(frame) = media.frames.lock().unwrap().get(&key) {
        return Ok(frame);
    }
    // FFmpeg returns the first frame at or after the seek point; aim half a
    // frame early so rounding can't skip to the next one.
    let seek = at.map(|(t, rate)| {
        (t.floor_to_frame(rate) - Time(rate.frame_duration().0 / 2)).max(Time::ZERO)
    });
    let mut frame = decode_one(media, path, seek, max_width)?;
    // Variable-rate files can end short of their reported duration.
    if frame.is_none()
        && let Some(t) = seek.filter(|t| *t > Time::ZERO)
    {
        let earlier = (t - Time::from_millis(1000)).max(Time::ZERO);
        frame = decode_one(media, path, Some(earlier), max_width)?;
    }
    let frame = frame.ok_or_else(|| {
        ServiceError(format!(
            "no frame at {} in {}",
            start.unwrap_or_default(),
            path.display()
        ))
    })?;
    media.frames.lock().unwrap().put(key, frame.clone());
    Ok(frame)
}

fn decode_one(
    media: &FfmpegMedia,
    path: &Path,
    t: Option<Time>,
    max_width: u32,
) -> ServiceResult<Option<Frame>> {
    let mut cmd = media.ffmpeg();
    if let Some(t) = t {
        // Before `-i`: seek to the keyframe, then decode up to `t`.
        cmd.args(["-ss", &secs_arg(t)]);
    }
    cmd.arg("-i")
        .arg(path)
        .args(["-map", "0:v:0", "-frames:v", "1", "-vf", &scale(max_width)]);
    pam_output(&mut cmd);
    let out = cmd.output().map_err(|e| err("ffmpeg", e))?;
    if !out.status.success() {
        return Err(ServiceError(format!(
            "decode {}: {}",
            path.display(),
            stderr_tail(&out.stderr)
        )));
    }
    read_pam(&mut out.stdout.as_slice()).map_err(|e| err("decode", e))
}

/// The proxy when one exists on disk, else the source.
pub(crate) fn video_source(item: &MediaItem) -> PathBuf {
    item.proxy
        .clone()
        .or_else(|| item.fingerprint.as_ref().map(proxy::proxy_path))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| item.path.clone())
}

/// Fit to `max_width` on square pixels, never upscaling; even sizes.
pub(crate) fn scale(max_width: u32) -> String {
    let w = if max_width == 0 { 16384 } else { max_width };
    format!(
        "scale=w='max(2,trunc(min(iw*sar,{w})/2)*2)':h='max(2,trunc(ow*ih/(iw*sar)/2)*2)':flags=bilinear,setsar=1"
    )
}

fn pam_output(cmd: &mut Command) {
    cmd.args([
        "-fps_mode",
        "passthrough",
        "-c:v",
        "pam",
        "-pix_fmt",
        "rgba",
        "-f",
        "image2pipe",
        "pipe:1",
    ]);
}

/// Next PAM image from the stream; `None` at a clean end.
fn read_pam(r: &mut impl BufRead) -> std::io::Result<Option<Frame>> {
    let bad = |m: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, m.to_string());
    let mut line = String::new();
    if r.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    if line.trim() != "P7" {
        return Err(bad("not a PAM stream"));
    }
    let (mut width, mut height, mut depth) = (0u32, 0u32, 0u32);
    loop {
        line.clear();
        if r.read_line(&mut line)? == 0 {
            return Err(bad("truncated PAM header"));
        }
        let mut parts = line.split_whitespace();
        match (parts.next(), parts.next().and_then(|v| v.parse().ok())) {
            (Some("ENDHDR"), _) => break,
            (Some("WIDTH"), Some(v)) => width = v,
            (Some("HEIGHT"), Some(v)) => height = v,
            (Some("DEPTH"), Some(v)) => depth = v,
            _ => {}
        }
    }
    if width == 0 || height == 0 || !(3..=4).contains(&depth) {
        return Err(bad("unsupported PAM image"));
    }
    let mut data = vec![0u8; (width * height * depth) as usize];
    r.read_exact(&mut data)?;
    if depth == 3 {
        data = data
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|&[r, g, b]| [r, g, b, 255])
            .collect();
    }
    Ok(Some(Frame {
        width,
        height,
        rgba: data.into(),
    }))
}

// ------------------------------------------------------------------ playback

pub(crate) fn playback(
    media: &FfmpegMedia,
    item: &MediaItem,
    from: Time,
    max_width: u32,
) -> ServiceResult<Box<dyn FrameStream>> {
    if item.kind == MediaKind::Image {
        let frame = still(media, &item.path, None, max_width)?;
        return Ok(Box::new(StillStream(Some((from, frame)))));
    }
    let info = media.info(item)?;
    let rate = info.video.as_ref().map(|v| v.rate).unwrap_or_default();
    let fd = rate.frame_duration();
    let first = from.max(Time::ZERO).floor_to_frame(rate);
    let mut stream = FfmpegFrames {
        ffmpeg: media.ffmpeg(),
        path: video_source(item),
        rate,
        first,
        max_width,
        hwaccel: true,
        frame_duration: fd,
        produced: 0,
        process: None,
    };
    stream.spawn()?;
    Ok(Box::new(stream))
}

struct StillStream(Option<(Time, Frame)>);

impl FrameStream for StillStream {
    fn next_frame(&mut self) -> ServiceResult<Option<(Time, Frame)>> {
        Ok(self.0.take())
    }
}

/// A long-lived FFmpeg piping PAM frames at the item's rate. Frame `n` is
/// at source time `first + n × frame duration`; the process dies with it.
struct FfmpegFrames {
    ffmpeg: Command,
    path: PathBuf,
    rate: Rate,
    first: Time,
    max_width: u32,
    hwaccel: bool,
    frame_duration: Time,
    produced: i64,
    process: Option<Process<BufReader<ChildStdout>>>,
}

impl FfmpegFrames {
    fn spawn(&mut self) -> ServiceResult<()> {
        let mut cmd = clone_command(&self.ffmpeg);
        if self.hwaccel {
            cmd.args(["-hwaccel", "auto"]);
        }
        // Start half a frame early so rounding can't drop the first frame,
        // rebase to zero and resample onto the item's frame grid.
        let seek = (self.first - Time(self.frame_duration.0 / 2)).max(Time::ZERO);
        let filters = format!(
            "setpts=PTS-STARTPTS,fps=fps={}/{}:round=near,{}",
            self.rate.num,
            self.rate.den,
            scale(self.max_width)
        );
        cmd.args(["-ss", &secs_arg(seek)])
            .arg("-i")
            .arg(&self.path)
            .args(["-map", "0:v:0", "-vf", &filters]);
        pam_output(&mut cmd);
        self.process = Some(Process::spawn(cmd, |out| {
            BufReader::with_capacity(1 << 20, out)
        })?);
        Ok(())
    }
}

impl FrameStream for FfmpegFrames {
    fn next_frame(&mut self) -> ServiceResult<Option<(Time, Frame)>> {
        let Some(process) = self.process.as_mut() else {
            return Ok(None);
        };
        let frame = match read_pam(&mut process.stdout) {
            Ok(Some(frame)) => frame,
            result => {
                let failure = process.finish();
                self.process = None;
                if let Some(message) = failure {
                    // Hardware decode refused before the first frame: retry in software.
                    if self.hwaccel && self.produced == 0 {
                        self.hwaccel = false;
                        self.spawn()?;
                        return self.next_frame();
                    }
                    return Err(ServiceError(format!(
                        "playback {}: {message}",
                        self.path.display()
                    )));
                }
                result.map_err(|e| err("playback", e))?;
                return Ok(None);
            }
        };
        let t = self.first + Time(self.frame_duration.0 * self.produced);
        self.produced += 1;
        Ok(Some((t, frame)))
    }
}

// --------------------------------------------------------------------- audio

pub(crate) fn audio(
    media: &FfmpegMedia,
    item: &MediaItem,
    stream: u32,
    from: Time,
    sample_rate: u32,
) -> ServiceResult<Box<dyn PcmStream>> {
    let map = audio_map(media, item, stream)?;
    let mut cmd = media.ffmpeg();
    cmd.args(["-ss", &secs_arg(from)]).arg("-i").arg(&item.path);
    pcm_output(&mut cmd, &map, sample_rate.max(1));
    Ok(Box::new(FfmpegPcm {
        process: Process::spawn(cmd, |out| out)?,
        buf: Vec::new(),
        carry: 0,
    }))
}

/// `-map` for an audio stream: `stream` is a container index as in
/// `AudioStream::index`; anything else counts audio streams from zero.
pub(crate) fn audio_map(
    media: &FfmpegMedia,
    item: &MediaItem,
    stream: u32,
) -> ServiceResult<String> {
    let info = media.info(item)?;
    if info.audio.iter().any(|a| a.index == stream) {
        Ok(format!("0:{stream}"))
    } else if (stream as usize) < info.audio.len() {
        Ok(format!("0:a:{stream}"))
    } else {
        Err(ServiceError(format!(
            "{} has no audio stream {stream}",
            item.path.display()
        )))
    }
}

/// Interleaved stereo f32le at `rate`.
pub(crate) fn pcm_output(cmd: &mut Command, map: &str, rate: u32) {
    cmd.args(["-map", map, "-vn", "-ac", "2", "-ar"])
        .arg(rate.to_string())
        .args(["-f", "f32le", "pipe:1"]);
}

struct FfmpegPcm {
    process: Process<ChildStdout>,
    buf: Vec<u8>,
    /// Bytes of an incomplete stereo frame kept at the start of `buf`.
    carry: usize,
}

impl PcmStream for FfmpegPcm {
    fn read(&mut self, out: &mut [f32]) -> ServiceResult<usize> {
        const FRAME: usize = 8;
        let want = (out.len() / 2) * FRAME;
        if want == 0 {
            return Ok(0);
        }
        self.buf.resize(want.max(self.carry), 0);
        let mut have = self.carry;
        while have < FRAME {
            let n = self
                .process
                .stdout
                .read(&mut self.buf[have..])
                .map_err(|e| err("audio", e))?;
            if n == 0 {
                return match self.process.finish() {
                    Some(message) => Err(ServiceError(format!("audio: {message}"))),
                    None => Ok(0),
                };
            }
            have += n;
        }
        let whole = have / FRAME * FRAME;
        for (o, b) in out.iter_mut().zip(self.buf[..whole].as_chunks::<4>().0) {
            *o = f32::from_le_bytes(*b);
        }
        self.buf.copy_within(whole..have, 0);
        self.carry = have - whole;
        Ok(whole / 4)
    }
}

// ------------------------------------------------------------------- process

/// A child whose stdout is read by the owner and whose stderr is drained by a
/// thread; killed on drop.
pub(crate) struct Process<R> {
    child: Child,
    pub stdout: R,
    stderr: Arc<Mutex<Vec<u8>>>,
}

impl<R> Process<R> {
    pub fn spawn(mut cmd: Command, wrap: impl FnOnce(ChildStdout) -> R) -> ServiceResult<Self> {
        let mut child = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| err("ffmpeg", e))?;
        let stdout = wrap(child.stdout.take().expect("piped stdout"));
        let stderr = Arc::new(Mutex::new(Vec::new()));
        if let Some(pipe) = child.stderr.take() {
            drain(pipe, stderr.clone());
        }
        Ok(Self {
            child,
            stdout,
            stderr,
        })
    }

    /// Wait for exit after stdout ended; the error message if it failed.
    pub fn finish(&mut self) -> Option<String> {
        match self.child.wait() {
            Ok(status) if status.success() => None,
            Ok(_) => {
                // Let the drain thread catch up with the last lines.
                std::thread::sleep(std::time::Duration::from_millis(20));
                Some(stderr_tail(&self.stderr.lock().unwrap()))
            }
            Err(e) => Some(e.to_string()),
        }
    }
}

impl<R> Drop for Process<R> {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Keep the last 8 KiB of stderr so FFmpeg never blocks on a full pipe.
fn drain(mut pipe: ChildStderr, into: Arc<Mutex<Vec<u8>>>) {
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = pipe.read(&mut buf) {
            if n == 0 {
                break;
            }
            let mut log = into.lock().unwrap();
            log.extend_from_slice(&buf[..n]);
            let excess = log.len().saturating_sub(8192);
            log.drain(..excess);
        }
    });
}

/// `Command` isn't `Clone`; rebuild it from its program and arguments.
fn clone_command(cmd: &Command) -> Command {
    let mut c = crate::command(Path::new(cmd.get_program()));
    c.args(cmd.get_args());
    c
}
