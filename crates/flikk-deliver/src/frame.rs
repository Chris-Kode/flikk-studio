//! Still export (PRD D3): one PNG composed like a render (through
//! `flikk_core::compose` when the media engine is there, else the graph),
//! with captions burned in by the same graph.

use crate::composed::{self, Frames};
use crate::ffmpeg::{self, ensure_free, filter_script_args, publish, temp_path};
use crate::graph::{self, GraphSpec};
use crate::preset::RenderSettings;
use crate::queue::{WorkDir, write_script};
use anyhow::Result;
use flikk_core::memory::CaptionStyle;
use flikk_core::model::Project;
use flikk_core::services::Services;
use flikk_core::{JobId, Time, TimeRange};
use std::ffi::OsString;
use std::path::Path;
use std::sync::atomic::AtomicBool;

/// Write the frame at timeline time `t` as a PNG at sequence size, with
/// captions burned in. Refuses to overwrite `path`.
pub fn export_frame(
    project: &Project,
    t: Time,
    caption_styles: &[CaptionStyle],
    services: &Services,
    path: &Path,
) -> Result<()> {
    ensure_free(path)?;
    let rate = project.timeline.settings.rate;
    let start = t.max(Time::ZERO).floor_to_frame(rate);
    let settings = RenderSettings {
        range: Some(TimeRange::new(start, start + rate.frame_duration())),
        ..RenderSettings::custom()
    };
    let mut spec = GraphSpec::new(project, &settings, caption_styles);
    spec.audio = false;
    spec.piped = services.media.is_some();
    composed::letter(project, &mut spec, services);
    let work = WorkDir::new()?;
    let g = graph::build(project, &spec, &work.0)?;
    let script = write_script(&work.0, "frame.txt", &g.script)?;
    let tmp = temp_path(path, &JobId::new().to_string());
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
    args.push(tmp.clone().into());
    let cancel = AtomicBool::new(false);
    let result = if spec.piped {
        let mut frames = Frames::new(project, services, &spec);
        composed::run(&args, &g, &mut frames, &|_| {}, &cancel)
    } else {
        ffmpeg::run(&args, 0.0, &|_| {}, &cancel).map(|_| ())
    };
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    publish(&tmp, path)
}
