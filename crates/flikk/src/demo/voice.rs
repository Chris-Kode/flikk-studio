//! `FLIKK_DEMO_VOICE=on|making`: Clean voice at 80 on the first dialogue
//! clip, selected, in the Audio workspace, with a stand-in cleanup engine
//! whose copy is made (`on`) or 42 % made (`making`). The switch is
//! enabled only with the Clean voice model in `FLIKK_HOME/models`.

use crate::state::EditorState;
use flikk_core::agent::Workspace;
use flikk_core::bus::Author;
use flikk_core::cleanup::{Progress, Treatment, VoiceCleanup};
use flikk_core::command::Command;
use flikk_core::model::{MediaItem, TrackKind};
use flikk_core::services::{CleanupService, ServiceError, ServiceResult};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// Answers every request with one state.
struct FakeCleanup(Progress);

impl CleanupService for FakeCleanup {
    fn request(&self, _: &MediaItem, _: u32, _: &Treatment) -> Progress {
        self.0.clone()
    }

    fn complete(
        &self,
        _: &MediaItem,
        _: u32,
        _: &Treatment,
        _: &dyn Fn(f32),
        _: &AtomicBool,
    ) -> ServiceResult<PathBuf> {
        Err(ServiceError("demo media cannot be cleaned".into()))
    }

    fn version(&self) -> u64 {
        0
    }
}

pub fn seed(state: &mut EditorState) {
    let progress = match std::env::var("FLIKK_DEMO_VOICE").as_deref() {
        Ok("on") => Progress::Made(PathBuf::new()),
        Ok("making") => Progress::Making(0.42),
        _ => return,
    };
    let project = state.bus.project();
    let Some(clip) = project
        .timeline
        .tracks
        .iter()
        .find(|t| t.kind == TrackKind::Audio)
        .and_then(|t| t.clips.first())
        .map(|c| c.id)
    else {
        return;
    };
    let command = Command::SetVoiceCleanup {
        clips: vec![clip],
        cleanup: VoiceCleanup::default(),
    };
    if let Err(e) = state.bus.execute(command, Author::User, "voice demo") {
        eprintln!("voice demo: {e}");
    }
    state.services.cleanup = Some(Arc::new(FakeCleanup(progress)));
    state.selection.clips = vec![clip];
    state.workspace = Workspace::Audio;
}
