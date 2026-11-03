//! `FLIKK_DEMO_PERSON=<file>`: a person mask on real footage. The
//! file (a video or a picture with people in it) sits over a gradient
//! with its background removed, selected, with the real media engine and
//! person masks (MODNet must be downloaded, or `FLIKK_MODNET` point at
//! it), the playhead a second in.

use crate::state::EditorState;
use anyhow::{Result, anyhow};
use flikk_core::bus::CommandBus;
use flikk_core::fx::{FxInstance, FxRef};
use flikk_core::mask::Mask;
use flikk_core::model::*;
use flikk_core::services::{MediaService, SubjectService};
use flikk_core::{ClipId, MediaId, Rate, Time};
use std::path::PathBuf;
use std::sync::Arc;

pub fn seed(state: &mut EditorState) {
    let Some(path) = std::env::var_os("FLIKK_DEMO_PERSON").map(PathBuf::from) else {
        return;
    };
    let media = match flikk_media::FfmpegMedia::new() {
        Ok(m) => Arc::new(m),
        Err(e) => return eprintln!("person demo: {e:#}"),
    };
    match project(media.as_ref(), path) {
        Ok((project, clip)) => {
            let mut bus = CommandBus::new(project);
            bus.set_permissions(state.settings.permissions.clone());
            state.bus = bus;
            state.preview = None;
            state.selection.clips = vec![clip];
            state.playhead = Time::from_secs_f64(1.0);
            state.services.subjects = Some(Arc::new(flikk_analysis::LocalSubjects::new(
                media.clone() as Arc<dyn MediaService>,
            )) as Arc<dyn SubjectService>);
            state.services.media = Some(media as Arc<dyn MediaService>);
        }
        Err(e) => eprintln!("person demo: {e:#}"),
    }
}

fn project(media: &dyn MediaService, path: PathBuf) -> Result<(Project, ClipId)> {
    let info = media.probe(&path).map_err(|e| anyhow!("{e}"))?;
    let still = info.duration <= Time::ZERO;
    let item = MediaItem {
        id: MediaId::new(),
        fingerprint: Some(media.fingerprint(&path).map_err(|e| anyhow!("{e}"))?),
        path,
        kind: if still {
            MediaKind::Image
        } else {
            MediaKind::Video
        },
        info: Some(info.clone()),
        bin: None,
        tags: Vec::new(),
        proxy: None,
        generated: None,
        tone_map: Default::default(),
    };
    let length = if still {
        Time::from_secs_f64(5.0)
    } else {
        info.duration
    };
    let mut person = Clip::new(
        Time::ZERO,
        length,
        ClipContent::Media {
            media: item.id,
            source_in: Time::ZERO,
            audio_stream: None,
        },
    );
    person.masks = vec![Mask::person()];
    let clip = person.id;
    let background = Clip::new(
        Time::ZERO,
        length,
        ClipContent::Generator {
            fx: FxInstance::new(FxRef::builtin("linear_gradient")),
        },
    );
    let mut project = Project::new("Person demo");
    project.timeline.settings.rate = Rate::FPS_30;
    project.timeline.tracks = vec![
        Track {
            clips: vec![person],
            ..Track::new(TrackKind::Video, "V2")
        },
        Track {
            clips: vec![background],
            ..Track::new(TrackKind::Video, "V1")
        },
    ];
    project.media = vec![item];
    Ok((project, clip))
}
