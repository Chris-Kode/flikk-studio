//! `FLIKK_DEMO_COMPOSE=<seconds>`: the compositor on real media. Test
//! sources are made with FFmpeg under the cache, and the project uses an
//! effect stack, a transition, GLSL and HTML generators, a blend mode, crop
//! and flip, a retimed clip and a disabled one, with the real engines and
//! the playhead at `<seconds>`. The project is also written next to the
//! media as `project.json`, so renders of it can be compared.

use crate::state::EditorState;
use anyhow::{Context as _, Result, bail};
use flikk_core::bus::CommandBus;
use flikk_core::fx::{FxInstance, FxRef, ParamValue, Transition};
use flikk_core::model::*;
use flikk_core::services::MediaService;
use flikk_core::{MediaId, Rate, Time, paths};
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::Arc;

pub fn seed(state: &mut EditorState) {
    let Some(at) = std::env::var("FLIKK_DEMO_COMPOSE")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
    else {
        return;
    };
    let media = match flikk_media::FfmpegMedia::new() {
        Ok(m) => Arc::new(m),
        Err(e) => return eprintln!("compositor demo: {e:#}"),
    };
    match project(media.as_ref()) {
        Ok(project) => {
            let mut bus = CommandBus::new(project);
            bus.set_permissions(state.settings.permissions.clone());
            state.bus = bus;
            state.preview = None;
            state.selection.clips.clear();
            state.playhead = Time::from_secs_f64(at);
            state.services.media = Some(media as Arc<dyn MediaService>);
        }
        Err(e) => eprintln!("compositor demo: {e:#}"),
    }
}

fn secs(s: f64) -> Time {
    Time::from_secs_f64(s)
}

/// `name` in `dir`, made by FFmpeg from lavfi `inputs` unless it exists.
fn source(dir: &Path, name: &str, inputs: &[&str]) -> Result<PathBuf> {
    let path = dir.join(name);
    if path.exists() {
        return Ok(path);
    }
    let mut cmd = Process::new("ffmpeg");
    cmd.args(["-y", "-loglevel", "error"]);
    for input in inputs {
        cmd.args(["-f", "lavfi", "-i", input]);
    }
    cmd.args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac"])
        .arg(&path);
    if !cmd.status().context("ffmpeg")?.success() {
        bail!("ffmpeg could not make {name}");
    }
    Ok(path)
}

fn item(media: &dyn MediaService, path: PathBuf) -> Result<MediaItem> {
    let info = media.probe(&path).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(MediaItem {
        id: MediaId::new(),
        path,
        kind: MediaKind::Video,
        fingerprint: None,
        info: Some(info),
        bin: None,
        tags: Vec::new(),
        proxy: None,
        generated: None,
        tone_map: Default::default(),
    })
}

fn media_clip(item: &MediaItem, start: f64, duration: f64, source_in: f64) -> Clip {
    Clip::new(
        secs(start),
        secs(duration),
        ClipContent::Media {
            media: item.id,
            source_in: secs(source_in),
            audio_stream: None,
        },
    )
}

fn generator(key: &str, start: f64, duration: f64) -> Clip {
    Clip::new(
        secs(start),
        secs(duration),
        ClipContent::Generator {
            fx: FxInstance::new(FxRef::builtin(key)),
        },
    )
}

fn track(kind: TrackKind, name: &str, clips: Vec<Clip>) -> Track {
    Track {
        clips,
        ..Track::new(kind, name)
    }
}

fn project(media: &dyn MediaService) -> Result<Project> {
    let dir = paths::cache().join("demo-compose");
    std::fs::create_dir_all(&dir)?;
    let a = source(
        &dir,
        "a.mp4",
        &["testsrc2=s=1280x720:r=30:d=12", "sine=f=440:d=12"],
    )?;
    let b = source(
        &dir,
        "b.mp4",
        &["testsrc=s=1280x720:r=30:d=20", "sine=f=660:d=20"],
    )?;
    let bars = source(&dir, "bars.mp4", &["smptebars=s=640x480:r=30:d=10"])?;
    let c = source(&dir, "c.mp4", &["rgbtestsrc=s=640x360:r=30:d=5"])?;
    let (a, b, bars, c) = (
        item(media, a)?,
        item(media, b)?,
        item(media, bars)?,
        item(media, c)?,
    );

    // V1: blurred and vignetted A, an iris into B at 2× with crop and
    // flip, then a disabled clip that must not show.
    let mut clip_a = media_clip(&a, 0.0, 4.0, 0.0);
    let mut blur = FxInstance::new(FxRef::builtin("gaussian_blur"));
    blur.params.insert("radius".into(), ParamValue::Number(4.0));
    clip_a.effects = vec![blur, FxInstance::new(FxRef::builtin("vignette"))];
    let mut clip_b = media_clip(&b, 4.0, 4.0, 2.0);
    clip_b.speed = 2.0;
    clip_b.transition_in = Some(Transition::new(FxRef::builtin("iris"), secs(1.0)));
    clip_b.transform.crop = [0.1, 0.05, 0.1, 0.05];
    clip_b.transform.flip = [true, false];
    let mut clip_c = media_clip(&c, 8.0, 1.0, 0.0);
    clip_c.enabled = false;

    // V2: a gradient over the second half, overlay-blended.
    let mut gradient = generator("linear_gradient", 6.0, 3.0);
    gradient.transform.blend = BlendMode::Overlay;
    gradient.transform.opacity = 0.8;

    // O1: bars picture-in-picture, rotated and screen-blended.
    let mut pip = media_clip(&bars, 0.5, 5.5, 0.0);
    pip.transform.scale = 0.35;
    pip.transform.position = [0.28, -0.26];
    pip.transform.rotation_deg = 8.0;
    pip.transform.blend = BlendMode::Screen;

    let mut project = Project::new("Compositor demo");
    project.timeline.settings.rate = Rate::FPS_30;
    project.timeline.tracks = vec![
        track(
            TrackKind::Title,
            "T1",
            vec![generator("lower_third", 1.0, 4.0)],
        ),
        track(TrackKind::Overlay, "O1", vec![pip]),
        track(TrackKind::Video, "V2", vec![gradient]),
        track(TrackKind::Video, "V1", vec![clip_a, clip_b, clip_c]),
        track(TrackKind::Audio, "A1", vec![media_clip(&a, 0.0, 8.0, 0.0)]),
    ];
    project.media = vec![a, b, bars, c];
    std::fs::write(
        dir.join("project.json"),
        serde_json::to_string_pretty(&project)?,
    )?;
    Ok(project)
}
