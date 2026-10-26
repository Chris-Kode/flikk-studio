use crate::state::EditorState;
use gpui_kit::*;

pub struct TranscriptPanel {
    state: Entity<EditorState>,
    _observe: Subscription,
}

impl TranscriptPanel {
    pub fn new(state: Entity<EditorState>, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let _observe = cx.observe(&state, |_, _, cx| cx.notify());
        Self { state, _observe }
    }
}

impl Render for TranscriptPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _ = &self.state;
        super::placeholder("Transcript", cx)
    }
}
