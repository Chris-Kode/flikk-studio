//! Tooltips: a short label, optionally with its shortcut.

use super::{kbd::Kbd, theme as t};
use gpui_kit::*;

pub struct Tooltip {
    text: SharedString,
    kbd: Option<SharedString>,
}

impl Tooltip {
    /// A builder for GPUI's `.tooltip(…)`.
    pub fn text(
        text: impl Into<SharedString>,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        Self::with_kbd(text, None::<SharedString>)
    }

    /// A builder for GPUI's `.tooltip(…)` showing `kbd` after the label.
    pub fn with_kbd(
        text: impl Into<SharedString>,
        kbd: Option<impl Into<SharedString>>,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        let text = text.into();
        let kbd = kbd.map(Into::into);
        move |_, cx| {
            cx.new(|_| Self {
                text: text.clone(),
                kbd: kbd.clone(),
            })
            .into()
        }
    }
}

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap_2()
            .h(px(24.))
            .px_2()
            .rounded(t::RADIUS)
            .bg(t::TIP)
            .border_1()
            .border_color(t::TIP_LN)
            .shadow(t::floating_shadow())
            .text_size(px(11.))
            .text_color(t::T1)
            .child(self.text.clone())
            .children(
                self.kbd
                    .clone()
                    .map(|k| div().text_color(t::T2).child(Kbd::new(k))),
            )
    }
}
