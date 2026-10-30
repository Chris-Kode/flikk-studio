//! Renders through the compositor: every output frame is composed by
//! [`koo_core::compose::program`] at render size and piped into FFmpeg as
//! raw RGBA, where the graph ([`GraphSpec::piped`]) burns in captions and
//! titles (so the compositor leaves them out) and builds the audio in the
//! same process.

use crate::ffmpeg::{self, command};
use crate::graph::{Graph, GraphSpec};
use anyhow::{Context as _, Result, anyhow, bail};
use koo_core::compose::{self, Pictures, Streams};
use koo_core::model::{MediaItem, Project};
use koo_core::services::{Frame, Services};
use koo_core::time::FLICKS_PER_SECOND;
use koo_core::{ClipId, Time, TimeRange};
use std::ffi::OsString;
use std::io::{Read as _, Write as _};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};

/// Whether `range` must be composited rather than drawn by the graph, and
/// can be (the media engine decodes the sources).
pub(crate) fn wanted(project: &Project, range: TimeRange, services: &Services) -> bool {
    services.media.is_some() && compose::needs_compositor(project, range)
}

/// Source pictures for a render: decoded in step with the frames from the
/// original files, never the playback proxies.
struct Originals(Streams);

impl Pictures for Originals {
    fn frame(&mut self, clip: ClipId, item: &MediaItem, t: Time) -> Option<Frame> {
        // The media engine finds proxies by path or fingerprint.
        let item = MediaItem {
            proxy: None,
            generated: None,
            fingerprint: None,
            ..item.clone()
        };
        self.0.frame(clip, &item, t)
    }

    fn finish(&mut self) {
        self.0.finish();
    }
}

/// Composes the frames of `spec`'s range in order.
pub(crate) struct Frames<'a> {
    project: &'a Project,
    /// Without lettering.
    services: Services,
    spec: &'a GraphSpec,
    pictures: Originals,
}

impl<'a> Frames<'a> {
    pub(crate) fn new(project: &'a Project, services: &'a Services, spec: &'a GraphSpec) -> Self {
        let pictures = Originals(Streams::new(services.media.clone(), spec.width, spec.rate));
        Self {
            project,
            services: Services {
                lettering: None,
                ..services.clone()
            },
            spec,
            pictures,
        }
    }

    /// Output frame `f` (from the range start).
    pub(crate) fn frame(&mut self, f: i64) -> Frame {
        let flicks = f as i128 * self.spec.rate.den as i128 * FLICKS_PER_SECOND as i128
            / self.spec.rate.num.max(1) as i128;
        let t = self.spec.range.start + Time(flicks as i64);
        compose::program(
            self.project,
            t,
            (self.spec.width, self.spec.height),
            &self.services,
            &mut self.pictures,
        )
    }
}

/// The input arguments for `spec`'s frames on stdin.
pub(crate) fn input_args(spec: &GraphSpec) -> Vec<OsString> {
    [
        "-f".to_string(),
        "rawvideo".into(),
        "-pix_fmt".into(),
        "rgba".into(),
        "-video_size".into(),
        format!("{}x{}", spec.width, spec.height),
        "-framerate".into(),
        format!("{}/{}", spec.rate.num, spec.rate.den),
        "-i".into(),
        "pipe:0".into(),
    ]
    .map(OsString::from)
    .to_vec()
}

/// Run FFmpeg with `args` (which read `pipe:0`, see [`input_args`]),
/// writing `graph.frames` frames from `frames`. `progress` gets the
/// fraction written; `cancel` kills FFmpeg between frames.
pub(crate) fn run(
    args: &[OsString],
    graph: &Graph,
    frames: &mut Frames<'_>,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> Result<()> {
    let mut child = command(ffmpeg::ffmpeg())
        .arg("-hide_banner")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("could not start {}", ffmpeg::ffmpeg().display()))?;
    let mut stderr = child.stderr.take().expect("piped");
    let errors = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    });
    let mut stdin = child.stdin.take().expect("piped");
    let total = graph.frames.max(1);
    let mut write_failed = false;
    for f in 0..total {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            bail!("canceled");
        }
        let frame = frames.frame(f);
        if stdin.write_all(&frame.rgba).is_err() {
            write_failed = true;
            break;
        }
        progress((f + 1) as f32 / total as f32);
    }
    drop(stdin);
    let status = child.wait()?;
    let stderr = errors.join().unwrap_or_default();
    if !status.success() || write_failed {
        let message = stderr
            .lines()
            .map(str::trim)
            .rfind(|l| !l.is_empty())
            .unwrap_or("ffmpeg failed");
        return Err(anyhow!(message.to_owned()));
    }
    Ok(())
}
