//! The pieces media is played with: play buttons in two looks and the
//! bars that show and set how far it played. Looks only: what plays is up
//! to the caller.

use super::{Button, Icon, IconName, theme as t};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::cell::Cell;
use std::rc::Rc;

/// A 3 px bar filled to `fraction` (clamped to 0..1) in `color`.
pub fn bar3(fraction: f32, color: Rgba) -> Div {
    div()
        .h(px(3.))
        .rounded_full()
        .bg(t::alpha(t::T1, 0.08))
        .child(
            div()
                .h_full()
                .rounded_full()
                .w(relative(fraction.clamp(0., 1.)))
                .bg(color),
        )
}

/// A bar at `fraction`; a click on it calls `seek` with where, 0..1.
pub fn seek_bar(
    fraction: f32,
    marked: Option<(f32, f32)>,
    seek: impl Fn(f32, &mut App) + 'static,
    _: &App,
) -> impl IntoElement {
    let bounds = Rc::new(Cell::new(Bounds::<Pixels>::default()));
    let seen = bounds.clone();
    div()
        .relative()
        .flex_1()
        .min_w_0()
        .h(px(14.))
        .flex()
        .items_center()
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, move |ev, _, cx| {
            let b = bounds.get();
            seek((ev.position.x - b.left()) / b.size.width, cx);
        })
        .child(
            canvas(move |b, _, _| seen.set(b), |_, _, _, _| {})
                .absolute()
                .size_full(),
        )
        .children(marked.map(|(a, b)| {
            // The marked part, under the position.
            let (a, b) = (a.clamp(0., 1.), b.clamp(0., 1.));
            div()
                .absolute()
                .left(relative(a))
                .w(relative((b - a).max(0.)))
                .h(px(9.))
                .rounded(px(2.))
                .bg(t::alpha(t::AC, 0.3))
        }))
        .child(bar3(fraction, t::T1).w_full())
}

/// The row play button's look, without what it plays.
pub fn play_button(id: impl Into<ElementId>, playing: bool) -> Button {
    Button::new(id)
        .ghost()
        .small()
        .icon(if playing {
            IconName::Pause
        } else {
            IconName::Play
        })
        .selected(playing)
}

/// The round play button's look, without what it plays.
pub fn play_dot(id: impl Into<ElementId>, playing: bool, size: Pixels) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(size)
        .rounded_full()
        .cursor_pointer()
        .when(playing, |d| d.bg(t::T1).text_color(t::G))
        .when(!playing, |d| {
            d.bg(t::HV).text_color(t::T1).hover(|s| s.bg(t::PS))
        })
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            Icon::new(if playing {
                IconName::Pause
            } else {
                IconName::Play
            })
            .size(size * 0.45),
        )
}
