//! Keyboard shortcut chips: `Esc`, `Cmd B`, `J K L`.

use super::theme as t;
use gpui_kit::*;

#[derive(IntoElement)]
pub struct Kbd {
    keys: SharedString,
    on_accent: bool,
}

impl Kbd {
    pub fn new(keys: impl Into<SharedString>) -> Self {
        Self {
            keys: keys.into(),
            on_accent: false,
        }
    }

    /// Sits on an accent or danger fill: a brighter outline.
    pub fn on_accent(mut self, on_accent: bool) -> Self {
        self.on_accent = on_accent;
        self
    }
}

impl RenderOnce for Kbd {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .flex()
            .flex_none()
            .items_center()
            .h(px(16.))
            .px(px(4.))
            .rounded(px(3.))
            .border_1()
            .border_color(t::alpha(t::ON_AC, if self.on_accent { 0.33 } else { 0.13 }))
            .text_size(px(10.))
            .font_weight(FontWeight::MEDIUM)
            .opacity(0.75)
            .child(self.keys)
    }
}
