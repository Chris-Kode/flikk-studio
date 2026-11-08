//! The user's effects, presets and sounds library, and the FX program
//! open in the code editor.

use super::EditorState;
use gpui_kit::*;

impl EditorState {
    /// Read the library again after it changed.
    pub fn reload_library(&mut self, cx: &mut Context<Self>) {
        self.library = flikk_core::library::Library::load();
        self.analyze_sounds(cx);
        cx.notify();
    }

    /// Analyze the library's sounds that have no results, so they can be
    /// found by how they sound. Without speech: it costs nothing.
    pub(super) fn analyze_sounds(&self, cx: &mut Context<Self>) {
        let Some(analysis) = self.services.analysis.clone() else {
            return;
        };
        let items: Vec<_> = self.library.sounds.iter().map(|s| s.item()).collect();
        cx.background_executor()
            .spawn(async move { items.iter().for_each(|i| analysis.analyze_sound(i)) })
            .detach();
    }

    /// Open `fx` in the code editor, or close it with `None`.
    pub fn edit_fx(&mut self, fx: Option<flikk_core::fx::FxRef>, cx: &mut Context<Self>) {
        if self.editing_fx != fx {
            self.editing_fx = fx;
            cx.notify();
        }
    }
}
