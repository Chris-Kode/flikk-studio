use crate::state::EditorState;
use gpui_kit::*;

pub struct ViewerPanel {
    state: Entity<EditorState>,
    _observe: Subscription,
}

impl ViewerPanel {
    pub fn new(state: Entity<EditorState>, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let _observe = cx.observe(&state, |_, _, cx| cx.notify());
        Self { state, _observe }
    }
}

impl Render for ViewerPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _ = &self.state;
        super::placeholder("Viewer", cx)
    }
}
