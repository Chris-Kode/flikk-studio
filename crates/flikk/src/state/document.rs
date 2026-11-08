//! The open project as a document: the file it is saved in, whether it has
//! unsaved changes, and what the autosaver holds of them for crash
//! recovery. Unsaved means "not at the undo position last saved", so
//! undoing back to the save makes it clean again.

use super::EditorState;
use crate::persistence;
use flikk_core::bus::CommandBus;
use gpui_kit::*;
use std::path::{Path, PathBuf};

/// The open project's file and how its unsaved changes stand.
pub(super) struct Document {
    /// The project's file; `None` until first saved.
    file: Option<PathBuf>,
    /// Undo position (see [`undo_position`]) as last saved or opened; `None`
    /// when the project opened with unsaved changes (recovered).
    saved: Option<u64>,
    /// What the autosaver was last told about the unsaved changes.
    autosaved: Autosaved,
}

impl Document {
    /// A project just opened from `file` (`None`: not saved anywhere yet)
    /// with history `bus`. A `recovered` one opens with unsaved changes,
    /// which its autosave already holds.
    pub(super) fn opened(file: Option<PathBuf>, recovered: bool, bus: &CommandBus) -> Self {
        let position = undo_position(bus);
        Self {
            file,
            saved: (!recovered).then_some(position),
            autosaved: if recovered {
                Autosaved::Kept(position)
            } else {
                Autosaved::Nothing
            },
        }
    }

    /// The project's file; `None` until first saved.
    pub(super) fn file(&self) -> Option<&Path> {
        self.file.as_deref()
    }

    /// Changes not saved to the file (or never saved at all), for the
    /// project of `bus`.
    pub(super) fn is_dirty(&self, bus: &CommandBus) -> bool {
        self.saved != Some(undo_position(bus))
    }

    /// Hand the unsaved changes of `bus`'s project to the autosaver, for
    /// crash recovery, unless it holds them already or the user threw them
    /// away. Undone back to the save, the stale autosave is dropped. The
    /// status line text when the autosaver last failed.
    pub(super) fn autosave(&mut self, bus: &CommandBus) -> Option<String> {
        if !self.is_dirty(bus) {
            // Undone back to the saved state: what was kept is stale.
            if matches!(self.autosaved, Autosaved::Kept(_)) {
                persistence::discard(bus.project().id);
                self.autosaved = Autosaved::Nothing;
            }
            return None;
        }
        let position = undo_position(bus);
        if self.autosaved.covers(position) {
            return None;
        }
        self.autosaved = Autosaved::Kept(position);
        persistence::autosave(bus.project(), self.file.as_deref());
        persistence::autosave_failure()
            .map(|e| format!("Unsaved changes could not be kept for recovery: {e}"))
    }

    /// The project of `bus` was saved to `path`, which is its file now.
    pub(super) fn saved_as(&mut self, path: PathBuf, bus: &CommandBus) {
        self.file = Some(path);
        self.saved = Some(undo_position(bus));
        self.autosaved = Autosaved::Nothing;
    }

    /// The user chose not to save: forget the unsaved changes kept for
    /// crash recovery, and keep them out of later autosaves (and the one
    /// written on quit). Edits made after this are kept as usual.
    pub(super) fn discard(&mut self, bus: &CommandBus) {
        persistence::discard(bus.project().id);
        self.autosaved = Autosaved::Discarded(undo_position(bus));
    }
}

/// Where the project is in its undo history: the version of the newest
/// undo step, 0 before any. Unlike the bus version, which every undo bumps,
/// it comes back to the same value when undo or redo returns to a state.
fn undo_position(bus: &CommandBus) -> u64 {
    bus.history().undo.last().map_or(0, |e| e.version)
}

/// What the autosaver was last told about the open project's unsaved
/// changes, by undo position.
#[derive(Clone, Copy)]
enum Autosaved {
    /// Nothing since the last save.
    Nothing,
    /// It holds the changes at this position.
    Kept(u64),
    /// The user threw away the changes at this position: they are not
    /// autosaved again.
    Discarded(u64),
}

impl Autosaved {
    /// Nothing needs to be handed over while the project is at `position`.
    fn covers(self, position: u64) -> bool {
        match self {
            Self::Nothing => false,
            Self::Kept(at) | Self::Discarded(at) => at == position,
        }
    }
}

// --- projects
impl EditorState {
    /// The open project's file; `None` until first saved.
    pub fn file(&self) -> Option<&Path> {
        self.document.file()
    }

    /// Changes not saved to the project's file (or never saved at all).
    /// Undoing back to the saved state makes it clean again.
    pub fn is_dirty(&self) -> bool {
        self.document.is_dirty(&self.bus)
    }

    /// Recently opened project files that still exist, newest first.
    pub fn recent_projects(&self) -> Vec<PathBuf> {
        persistence::recent()
            .into_iter()
            .filter(|p| p.exists())
            .collect()
    }

    /// Hand unsaved changes to the autosaver, for crash recovery.
    pub(super) fn autosave(&mut self) {
        if let Some(failure) = self.document.autosave(&self.bus) {
            self.status = Some(failure);
        }
    }

    /// Replace the open project with a new one saved at `path`, which names
    /// it, with a sequence of `settings`. Unsaved changes must be settled
    /// first (see `crate::projects`). Whether it saved.
    pub fn create_project(
        &mut self,
        path: PathBuf,
        settings: flikk_core::model::SequenceSettings,
        cx: &mut Context<Self>,
    ) -> bool {
        let name = flikk_store::project_file::name_of(&path);
        self.switch_to(persistence::new_project(&name, settings), cx);
        self.save_project(path, cx)
    }

    /// Open a project file in place of the open project; unsaved changes
    /// must be settled first. Whether it opened.
    pub fn open_project(&mut self, path: &Path, cx: &mut Context<Self>) -> bool {
        if self.file() == Some(path) {
            return true;
        }
        match persistence::open(path) {
            Some(loaded) => {
                self.switch_to(loaded, cx);
                true
            }
            None => {
                if !path.exists() {
                    persistence::forget_recent(path);
                }
                self.status = persistence::take_notice();
                cx.notify();
                false
            }
        }
    }

    /// Save the project to `path`, which becomes its file (Save As when it
    /// differs). The project takes the file's name. Whether it saved.
    pub fn save_project(&mut self, path: PathBuf, cx: &mut Context<Self>) -> bool {
        let name = flikk_store::project_file::name_of(&path);
        if self.bus.project().name != name {
            self.bus.rename_project(&name);
        }
        let saved = persistence::save(&path, self.bus.project());
        self.proposals.sync(&self.bus);
        if saved {
            self.document.saved_as(path, &self.bus);
            self.status = None;
        } else {
            self.status = persistence::take_notice();
        }
        cx.notify();
        saved
    }

    /// The user chose not to save: forget the unsaved changes kept for
    /// crash recovery, and keep them out of later autosaves (and the one
    /// written on quit). Edits made after this are kept as usual.
    pub fn discard_changes(&mut self) {
        self.document.discard(&self.bus);
    }
}
