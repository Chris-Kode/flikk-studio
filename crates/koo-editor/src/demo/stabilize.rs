//! `KOOEDITOR_DEMO_STABILIZE=<seconds>`: a shaky handheld stand-in, made
//! with FFmpeg under the cache (a still with random crop jitter, a slow
//! pan, a small wobble and a picture moving across it), on a timeline of
//! its own with stabilization on, selected, its camera path measured, and
//! the playhead at `<seconds>`. `KOOEDITOR_DEMO_STABILIZE_MODE` picks
//! `off`, `locked` or `none` (Smooth without crop) instead.

use crate::state::EditorState;
use anyhow::{Context as _, Result, bail};
use koo_core::bus::CommandBus;
use koo_core::model::*;
use koo_core::services::MediaService;
use koo_core::stabilize::{Stabilization, StabilizeCrop, StabilizeMode};
use koo_core::{MediaId, Time, paths};
use std::process::Command as Process;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// The shaky stand-in: lavfi inputs and the filter that shakes them.
const INPUTS: [&str; 2] = [
    "mandelbrot=size=1600x1000:start_x=-0.743643887037151:start_y=0.13182590420533:start_scale=0.005:end_scale=0.005:rate=30",
    "testsrc2=size=360x240:rate=30",
];
const SHAKE: &str = "[0][1]overlay=x='600+200*sin(t*1.3)':y=420,\
rotate='0.012*sin(t*9.1)+0.006*sin(t*13.7)':c=black,\
crop=1280:720:'160+60*t/6+18*sin(n*1.1)+12*(random(0)-0.5)':'140+16*sin(n*1.7+1)+12*(random(1)-0.5)'";

pub fn seed(state: &mut EditorState) {
    let Some(at) = std::env::var("KOOEDITOR_DEMO_STABILIZE")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
    else {
        return;
    };
    let media = match koo_media::FfmpegMedia::new() {
        Ok(m) => Arc::new(m),
        Err(e) => return eprintln!("stabilization demo: {e:#}"),
    };
    match project(media.as_ref()) {
        Ok((project, clip)) => {
            if let (Some(stabilizer), Some(item)) =
                (&state.services.stabilizer, project.media.first())
                && let Err(e) = stabilizer.complete(item, &|_| {}, &AtomicBool::new(false))
            {
                eprintln!("stabilization demo: {e}");
            }
            let mut bus = CommandBus::new(project);
            bus.set_permissions(state.settings.permissions.clone());
            state.bus = bus;
            state.preview = None;
            state.selection.clips = vec![clip];
            state.playhead = Time::from_secs_f64(at);
            state.services.media = Some(media as Arc<dyn MediaService>);
        }
        Err(e) => eprintln!("stabilization demo: {e:#}"),
    }
}

fn project(media: &dyn MediaService) -> Result<(Project, koo_core::ClipId)> {
    let dir = paths::cache().join("demo-stabilize");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("shaky.mp4");
    if !path.exists() {
        let mut cmd = Process::new("ffmpeg");
        cmd.args(["-y", "-loglevel", "error"]);
        for input in INPUTS {
            cmd.args(["-f", "lavfi", "-i", input]);
        }
        cmd.args(["-filter_complex", SHAKE, "-t", "6"])
            .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
            .arg(&path);
        if !cmd.status().context("ffmpeg")?.success() {
            bail!("ffmpeg could not make the shaky clip");
        }
    }
    let info = media.probe(&path).map_err(|e| anyhow::anyhow!("{e}"))?;
    let fingerprint = media
        .fingerprint(&path)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let item = MediaItem {
        id: MediaId::new(),
        path,
        kind: MediaKind::Video,
        fingerprint: Some(fingerprint),
        info: Some(info.clone()),
        bin: None,
        tags: Vec::new(),
        proxy: None,
        generated: None,
        tone_map: Default::default(),
    };
    let mut clip = Clip::new(
        Time::ZERO,
        info.duration,
        ClipContent::Media {
            media: item.id,
            source_in: Time::ZERO,
            audio_stream: None,
        },
    );
    let mode = std::env::var("KOOEDITOR_DEMO_STABILIZE_MODE").unwrap_or_default();
    clip.stabilization = Stabilization {
        enabled: mode != "off",
        mode: if mode == "locked" {
            StabilizeMode::Locked
        } else {
            StabilizeMode::Smooth
        },
        crop: if mode == "none" {
            StabilizeCrop::None
        } else {
            StabilizeCrop::Auto
        },
        ..Stabilization::default()
    };
    let id = clip.id;
    let mut project = Project::new("Stabilization demo");
    project.timeline.settings.width = 1280;
    project.timeline.settings.height = 720;
    project.timeline.settings.rate = koo_core::Rate::FPS_30;
    project.timeline.tracks = vec![Track {
        clips: vec![clip],
        ..Track::new(TrackKind::Video, "V1")
    }];
    project.media.push(item);
    Ok((project, id))
}
