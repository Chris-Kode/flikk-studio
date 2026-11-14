//! A scene as a video file: the clip a video model follows.

use super::renderer::{Renderer, Turns};
use super::{Detail, FPS, Look, Scene};
use flikk_os::process::{ChildGuard, command, ffmpeg};
use std::io::Write as _;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};

/// The longer side of a blockout's clip, in pixels: enough for a model to
/// read staging and camera from, small enough for every provider's upload.
const LONG_SIDE: f64 = 1280.0;
/// What writing a clip is called among the jobs on the GPU.
const JOB: &str = "Blockout clip";

impl Scene {
    /// The size its clip is written at for a picture `aspect` wide to one
    /// high (16:9 is 1.78): [`LONG_SIDE`] on the longer side, even numbers.
    pub fn clip_size(aspect: f64) -> (u32, u32) {
        let aspect = if aspect.is_finite() && aspect > 0.0 {
            aspect.clamp(0.25, 4.0)
        } else {
            16.0 / 9.0
        };
        let even = |n: f64| ((n / 2.0).round() as u32 * 2).max(2);
        if aspect >= 1.0 {
            (even(LONG_SIDE), even(LONG_SIDE / aspect))
        } else {
            (even(LONG_SIDE * aspect), even(LONG_SIDE))
        }
    }

    /// Write the scene to `path` as an H.264 MP4 of `size` at [`FPS`], its
    /// folder made when missing, its frames drawn by `renderer` (a job on
    /// the GPU's list while they go there). `cancel` stops it between
    /// frames and while a frame waits for the GPU; stopped or failed,
    /// nothing is left at `path`.
    pub fn write_clip(
        &self,
        renderer: &Renderer,
        size: (u32, u32),
        path: &Path,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        let fail = |e: &dyn std::fmt::Display| format!("could not write the blockout's clip: {e}");
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder).map_err(|e| fail(&e))?;
        }
        let part = flikk_os::fs::temp_sibling(path);
        let written = self.encode(renderer, size, &part, cancel);
        let placed = written.and_then(|()| std::fs::rename(&part, path).map_err(|e| fail(&e)));
        if placed.is_err() {
            let _ = std::fs::remove_file(&part);
        }
        placed
    }

    fn encode(
        &self,
        renderer: &Renderer,
        (width, height): (u32, u32),
        part: &Path,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        let fail = |e: &dyn std::fmt::Display| format!("could not write the blockout's clip: {e}");
        // Guarded: every early return below kills and reaps ffmpeg.
        let mut child = ChildGuard::spawn(
            command(ffmpeg())
                .args(["-hide_banner", "-loglevel", "error", "-y"])
                .args(["-f", "rawvideo", "-pix_fmt", "rgba"])
                .args(["-video_size", &format!("{width}x{height}")])
                .args(["-framerate", &FPS.to_string(), "-i", "pipe:0"])
                .args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "20"])
                .args(["-pix_fmt", "yuv420p", "-movflags", "+faststart"])
                .args(["-f", "mp4"])
                .arg(part)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        )
        .map_err(|e| fail(&e))?;
        let mut frames = child
            .stdin
            .take()
            .ok_or_else(|| fail(&"no pipe to FFmpeg"))?;
        let mut job = None;
        let count = self.frames();
        for frame in 0..count {
            if cancel.load(Ordering::Relaxed) {
                return Err("stopped".into());
            }
            let at = frame as f64 / FPS as f64;
            let turns = Turns::Listed {
                job: &mut job,
                label: JOB,
                cancel,
            };
            let plan = self.plan(at, (width, height), Look::Shot(Detail::Full));
            let Some(picture) = renderer.draw(&plan, turns) else {
                return Err("stopped".into());
            };
            frames.write_all(&picture.rgba).map_err(|e| fail(&e))?;
            if let Some(job) = &job {
                job.progress((frame + 1) as f32 / count.max(1) as f32);
            }
        }
        drop(frames);
        match child.wait() {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => Err(fail(&format!("FFmpeg ended with {status}"))),
            Err(e) => Err(fail(&e)),
        }
    }
}
