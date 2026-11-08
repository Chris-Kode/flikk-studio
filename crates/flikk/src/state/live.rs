//! What the viewer shows in place of the committed project: an edit
//! being dragged (not committed), else a change set's draft.

use super::EditorState;
use flikk_core::ChangeSetId;
use flikk_core::command::Command;
use flikk_core::model::Project;
use gpui_kit::*;

impl EditorState {
    /// What the viewer shows: a control being dragged, else the previewed
    /// change set's draft, else the committed project.
    pub fn display_project(&self) -> &Project {
        if let Some(live) = &self.live {
            return live;
        }
        self.preview
            .and_then(|id| self.bus.change_set(id))
            .and_then(|set| set.draft.as_ref())
            .unwrap_or(self.bus.project())
    }

    /// Show `commands` applied in the viewer without committing them; none
    /// clears it. Any committed edit clears it too.
    pub fn show_live(&mut self, commands: &[Command], cx: &mut Context<Self>) {
        let last = self.live.take();
        if commands.is_empty() {
            if last.is_some() {
                self.live_version += 1;
                cx.notify();
            }
            return;
        }
        let mut project = self.display_project().clone();
        let applied = commands.iter().all(|c| {
            c.plan(&project)
                .and_then(|plan| flikk_core::ops::apply_all(&mut project, &plan.ops))
                .is_ok()
        });
        // Not applicable: keep the last good preview.
        self.live = if applied { Some(project) } else { last };
        self.live_version += 1;
        cx.notify();
    }

    /// Drop both drag previews, e.g. when what they were dragged on is
    /// replaced.
    pub(super) fn clear_live(&mut self) {
        if self.live.take().is_some() {
            self.live_version += 1;
        }
        self.captions.clear_live();
    }

    pub fn set_preview(&mut self, preview: Option<ChangeSetId>, cx: &mut Context<Self>) {
        self.preview = preview;
        cx.notify();
    }
}
