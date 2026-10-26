//! Agent settings (PRD "Agent configuration"), opened from the chat panel.

use crate::state::EditorState;
use gpui_kit::*;

pub struct SettingsPanel {
    state: Entity<EditorState>,
    _observe: Subscription,
}

impl SettingsPanel {
    pub fn new(state: Entity<EditorState>, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let _observe = cx.observe(&state, |_, _, cx| cx.notify());
        Self { state, _observe }
    }
}

impl Render for SettingsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _ = &self.state;
        super::placeholder("Agent settings", cx)
    }
}
