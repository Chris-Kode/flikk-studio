//! Still export (PRD D3): one PNG composed like a render (through
//! `flikk_core::compose` when the media engine is there, else the graph),
//! with captions burned in by the same graph.

use crate::composed::{self, Frames};
use crate::ffmpeg::{self, TempOutput, ensure_free, filter_script_args, publish_exact};
use crate::graph::{self, GraphSpec};
use crate::preset::RenderSettings;
use crate::queue::{WorkDir, write_script};
use anyhow::{Result, bail};
use flikk_core::memory::CaptionStyle;
use flikk_core::model::Project;
use flikk_core::services::Services;
use flikk_core::{Time, TimeRange};
use std::ffi::OsString;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// Write the frame at timeline time `t` as a PNG at sequence size, with
/// captions burned in. Refuses to overwrite `path`. Never canceled; see
/// [`export_frame_until`].
pub fn export_frame(
    project: &Project,
    t: Time,
    caption_styles: &[CaptionStyle],
    services: &Services,
    path: &Path,
) -> Result<()> {
    let never = AtomicBool::new(false);
    export_frame_until(project, t, caption_styles, services, path, &never)
}

/// [`export_frame`], failing with "canceled" once `cancel` is set: FFmpeg
/// is killed, and nothing is written to `path`.
pub fn export_frame_until(
    project: &Project,
    t: Time,
    caption_styles: &[CaptionStyle],
    services: &Services,
    path: &Path,
    cancel: &AtomicBool,
) -> Result<()> {
    check(cancel)?;
    ensure_free(path)?;
    let rate = project.timeline.settings.rate;
    let start = t.max(Time::ZERO).floor_to_frame(rate);
    let settings = RenderSettings {
        range: Some(TimeRange::new(start, start + rate.frame_duration())),
        ..RenderSettings::custom()
    };
    let mut spec = GraphSpec::new(project, &settings, caption_styles);
    spec.audio = false;
    composed::route(project, &mut spec, services, services.media.is_some());
    let work = WorkDir::new()?;
    let g = graph::build(project, &spec, &work.0)?;
    check(cancel)?;
    let script = write_script(&work.0, "frame.txt", &g.script)?;
    // Removed however the export ends, once published or not.
    let tmp = TempOutput::new(path);
    let mut args: Vec<OsString> = vec!["-loglevel".into(), "error".into(), "-y".into()];
    if spec.piped {
        args.extend(composed::input_args(&spec));
    }
    args.extend(filter_script_args(&script));
    args.extend(
        [
            "-map",
            "[vout]",
            "-frames:v",
            "1",
            "-update",
            "1",
            "-c:v",
            "png",
            "-f",
            "image2",
        ]
        .map(OsString::from),
    );
    args.push(tmp.path().into());
    if spec.piped {
        let mut frames = Frames::new(project, services, &spec, path);
        composed::run(&args, &g, &mut frames, &|_| {}, cancel)?;
    } else {
        ffmpeg::run(&args, 0.0, &|_| {}, cancel)?;
    }
    // The one frame is composed whole, so a cancel while it was drawn is
    // only seen here: the temp file goes, unpublished.
    check(cancel)?;
    publish_exact(&tmp, path)
}

/// Fail with "canceled", like renders, once `cancel` is set.
fn check(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        bail!("canceled");
    }
    Ok(())
}
