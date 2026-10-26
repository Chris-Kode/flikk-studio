//! Panels. Each file owns one panel's internals; `app.rs` owns the layout.
//! Every panel is built with `new(state, window, cx)`, observes the shared
//! [`EditorState`](crate::state::EditorState) and renders itself.

pub mod chat;
pub mod deliver;
pub mod inspector;
pub mod media_pool;
pub mod settings;
pub mod timeline;
pub mod transcript;
pub mod viewer;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;

/// Drag payload for pool media dropped on the timeline or viewer.
#[derive(Clone, Debug)]
pub struct MediaDrag {
    pub media: koo_core::MediaId,
    pub name: SharedString,
}

impl Render for MediaDrag {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .text_xs()
            .bg(cx.theme().secondary)
            .child(self.name.clone())
    }
}

/// Stand-in body for a panel that has not been built yet.
pub fn placeholder(name: &'static str, cx: &App) -> Div {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(name)
}
