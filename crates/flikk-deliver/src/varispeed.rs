//! The sound of a clip retimed by a hair to correct clock drift, rendered
//! through the preview's own resampler ([`flikk_core::sync::varispeed`]) so
//! export and preview put every sample at the same time: the source is
//! decoded as the preview decodes it (seeked input, stereo f32 at the
//! output rate), resampled, and written to lossless FLAC in the job's
//! work folder for the graph to read.

use anyhow::{Context as _, Result, bail};
use flikk_core::model::MediaItem;
use flikk_core::sync::varispeed::Varispeed;
use flikk_os::process::{ChildGuard, command, ffmpeg};
use std::io::{Read, Write};
use std::path::Path;
use std::process::Stdio;

/// `seconds` of output from source time `from` of `item`'s audio
/// `stream` (a container index, else the n-th audio stream, as the
/// preview takes it), played at `speed`, at `rate`; written to
/// `out`.
pub fn render(
    item: &MediaItem,
    stream: Option<u32>,
    from: f64,
    seconds: f64,
    speed: f64,
    rate: u32,
    out: &Path,
) -> Result<()> {
    let n = stream.unwrap_or(0);
    let audio = item
        .info
        .as_ref()
        .map(|i| i.audio.as_slice())
        .unwrap_or(&[]);
    let map = if audio.iter().any(|a| a.index == n) {
        format!("0:{n}")
    } else {
        format!("0:a:{n}")
    };
    // Both guarded: an early return (a failed write, a missing pipe) kills
    // and reaps them.
    let mut decode = ChildGuard::spawn(
        command(ffmpeg())
            .args([
                "-v",
                "error",
                "-nostdin",
                "-ss",
                &format!("{:.6}", from.max(0.0)),
            ])
            .arg("-i")
            .arg(&item.path)
            .args(["-map", &map, "-vn", "-ac", "2", "-ar", &rate.to_string()])
            .args(["-f", "f32le", "pipe:1"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null()),
    )
    .context("could not run ffmpeg")?;
    let mut encode = ChildGuard::spawn(
        command(ffmpeg())
            .args(["-v", "error", "-nostdin", "-y", "-f", "f32le", "-ac", "2"])
            .args(["-ar", &rate.to_string(), "-i", "pipe:0"])
            .args(["-c:a", "flac", "-sample_fmt", "s32"])
            .arg(out)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
    )
    .context("could not run ffmpeg")?;
    let mut input = decode.stdout.take().context("ffmpeg gave no output")?;
    let mut sink = encode.stdin.take().context("ffmpeg took no input")?;
    let mut bytes = Vec::new();
    let mut pull = |o: &mut [f32]| {
        bytes.resize(o.len() * 4, 0);
        let mut got = 0;
        while got < bytes.len() {
            match input.read(&mut bytes[got..]) {
                Ok(0) | Err(_) => break,
                Ok(k) => got += k,
            }
        }
        let samples = got / 4;
        for (s, b) in o.iter_mut().zip(bytes[..samples * 4].as_chunks::<4>().0) {
            *s = f32::from_le_bytes(*b);
        }
        samples
    };
    let mut vary = Varispeed::new(speed);
    let mut left = (seconds * rate as f64).ceil() as usize;
    let mut block = vec![0f32; 8192];
    let mut raw = Vec::with_capacity(block.len() * 4);
    while left > 0 {
        let frames = left.min(block.len() / 2);
        vary.read(&mut block[..frames * 2], &mut pull);
        raw.clear();
        for s in &block[..frames * 2] {
            raw.extend_from_slice(&s.to_le_bytes());
        }
        sink.write_all(&raw)
            .context("ffmpeg stopped taking audio")?;
        left -= frames;
    }
    drop(sink);
    let _ = decode.kill();
    let _ = decode.wait();
    if !encode.wait()?.success() {
        bail!("ffmpeg could not write {}", out.display());
    }
    Ok(())
}
