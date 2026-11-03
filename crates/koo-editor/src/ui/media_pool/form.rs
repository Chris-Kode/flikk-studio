//! The rows of a form dialog (Video, Voiceover, Generate, Voices): a 96 px
//! label column beside its control, a slider with its value, and the line
//! that says why the form can't go ahead.

use crate::ui::kit::{Icon, IconName, Slider, h_flex, theme as t};
use gpui_kit::base::slider::SliderState;
use gpui_kit::*;

/// `label` beside `control`.
pub(crate) fn row(label: impl Into<SharedString>, control: impl IntoElement) -> Div {
    h_flex()
        .gap(px(10.))
        .min_h(px(24.))
        .child(
            div()
                .w(px(96.))
                .flex_none()
                .text_color(t::T2)
                .child(label.into()),
        )
        .child(div().flex_1().min_w_0().child(control))
}

/// A slider with its value, mono, at the right.
pub(crate) fn slider(state: &Entity<SliderState>, value: impl Into<SharedString>) -> Div {
    h_flex()
        .gap(px(8.))
        .child(div().flex_1().min_w_0().child(Slider::new(state)))
        .child(
            h_flex()
                .w(px(40.))
                .flex_none()
                .justify_end()
                .font_family(t::mono())
                .text_size(px(11.))
                .text_color(t::T2)
                .child(value.into()),
        )
}

/// Why the form can't go ahead.
pub(crate) fn problem(text: impl Into<SharedString>) -> Div {
    h_flex()
        .items_start()
        .gap(px(8.))
        .text_color(t::ERR)
        .child(
            div()
                .flex_none()
                .pt(px(1.))
                .child(Icon::new(IconName::CircleAlert).size(px(14.))),
        )
        .child(div().flex_1().min_w_0().child(text.into()))
}
