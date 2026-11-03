//! The 4 px progress bar: a signal-blue fill for a known fraction, or a
//! fading segment that sweeps the track while the length is unknown.

use super::theme as t;
use gpui_kit::*;
use std::time::Duration;

/// Share of the track the indeterminate segment covers.
const SEGMENT: f32 = 0.34;

#[derive(IntoElement)]
pub struct Progress {
    value: Option<f32>,
    style: StyleRefinement,
}

impl Progress {
    pub fn new() -> Self {
        Self {
            value: Some(0.),
            style: StyleRefinement::default(),
        }
    }

    /// The fraction done, 0–1.
    pub fn value(mut self, fraction: f32) -> Self {
        self.value = Some(fraction.clamp(0., 1.));
        self
    }

    /// Working, length unknown.
    pub fn indeterminate(mut self) -> Self {
        self.value = None;
        self
    }
}

impl Default for Progress {
    fn default() -> Self {
        Self::new()
    }
}

impl Styled for Progress {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Progress {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let mut track = div()
            .relative()
            .w_full()
            .h(px(4.))
            .rounded_full()
            .overflow_hidden()
            .bg(t::R1);
        track.style().refine(&self.style);
        let fill = div().absolute().top_0().bottom_0().rounded_full();
        match self.value {
            Some(fraction) => track.child(fill.left_0().w(relative(fraction)).bg(t::AC)),
            None => {
                let clear = t::alpha(t::AC, 0.);
                let half = |from: Rgba, to: Rgba| {
                    div().flex_1().h_full().bg(linear_gradient(
                        90.,
                        linear_color_stop(from, 0.),
                        linear_color_stop(to, 1.),
                    ))
                };
                track.child(
                    fill.flex()
                        .w(relative(SEGMENT))
                        .child(half(clear, t::AC))
                        .child(half(t::AC, clear))
                        .with_animation(
                            "progress",
                            Animation::new(Duration::from_millis(1400))
                                .repeat()
                                .with_easing(ease_in_out),
                            |el, delta| el.left(relative(-SEGMENT + delta * (1. + SEGMENT))),
                        ),
                )
            }
        }
    }
}
