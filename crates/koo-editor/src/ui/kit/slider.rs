//! Sliders: a 3 px track with an accent fill and a 10 px thumb, optionally
//! under a header row (label left, mono value right), or bipolar with the
//! fill running from a centre tick. Vertical sliders are mixer faders: a
//! recessed groove and a fader cap. GPUI Base's [`SliderState`] keeps the
//! value and the drag; this element only draws it.

use super::theme as t;
use gpui_kit::base::slider::SliderState;
use gpui_kit::base::{
    Slider as BaseSlider, SliderIndicator, SliderThumb, SliderTrack, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

fn shadow(color: Rgba, y: f32, blur: f32, spread: f32, inset: bool) -> BoxShadow {
    BoxShadow {
        color: color.into(),
        offset: point(px(0.), px(y)),
        blur_radius: px(blur),
        spread_radius: px(spread),
        inset,
    }
}

/// The thumb's drop shadow, under a ring of `ring` when it is hovered or
/// held.
fn thumb_shadow(ring: Option<Rgba>) -> Vec<BoxShadow> {
    let drop = shadow(t::alpha(t::hex(0x000000), 0.53), 1., 2., 0., false);
    match ring {
        Some(ring) => vec![shadow(ring, 0., 0., 4., false), drop],
        None => vec![drop],
    }
}

/// Track whether the pointer holds the slider, for the dragging look.
fn hold<E>(held: &Entity<bool>, pressed: bool) -> impl Fn(&E, &mut Window, &mut App) + 'static {
    let held = held.clone();
    move |_, _, cx| {
        held.update(cx, |h, cx| {
            if *h != pressed {
                *h = pressed;
                cx.notify();
            }
        })
    }
}

#[derive(IntoElement)]
pub struct Slider {
    state: Entity<SliderState>,
    vertical: bool,
    bipolar: bool,
    disabled: bool,
    label: Option<SharedString>,
    readout: Option<SharedString>,
    style: StyleRefinement,
}

impl Slider {
    pub fn new(state: &Entity<SliderState>) -> Self {
        Self {
            state: state.clone(),
            vertical: false,
            bipolar: false,
            disabled: false,
            label: None,
            readout: None,
            style: StyleRefinement::default(),
        }
    }

    /// A mixer fader: bottom to top, with a fader cap.
    pub fn vertical(mut self) -> Self {
        self.vertical = true;
        self
    }

    /// The fill runs from a centre tick to the thumb, for ranges centred on
    /// zero such as pan or exposure.
    pub fn bipolar(mut self) -> Self {
        self.bipolar = true;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The header's left side.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// The header's right side: the value as shown, with its unit.
    pub fn readout(mut self, value: impl Into<SharedString>) -> Self {
        self.readout = Some(value.into());
        self
    }
}

impl Styled for Slider {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Slider {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let id = self.state.entity_id();
        let group = SharedString::from(format!("flikk-slider-{id}"));
        let held = window.use_keyed_state(("flikk-slider", id), cx, |_, _| false);
        let dragging = *held.read(cx) && !self.disabled;
        let p = self.state.read(cx).percentage().end;
        let axis = if self.vertical {
            Axis::Vertical
        } else {
            Axis::Horizontal
        };

        let track = SliderTrack::new(&self.state)
            .axis(axis)
            .disabled(self.disabled)
            .relative()
            .flex()
            .when(!self.disabled, |s| {
                s.capture_any_mouse_down(hold(&held, true))
                    .on_mouse_up(MouseButton::Left, hold(&held, false))
                    .on_mouse_up_out(MouseButton::Left, hold(&held, false))
            });

        let slider = if self.vertical {
            let cap = SliderThumb::new(&self.state)
                .axis(axis)
                .disabled(self.disabled)
                .absolute()
                .bottom(relative(p))
                .left(px(-10.))
                .mb(px(-6.))
                .w(px(24.))
                .h(px(12.))
                .rounded(px(3.))
                .bg(t::LN_HV)
                .shadow(vec![
                    shadow(t::alpha(t::hex(0x000000), 0.67), 2., 5., 0., false),
                    shadow(t::alpha(t::ON_AC, 0.15), 1., 0., 0., true),
                ])
                .child(
                    div()
                        .absolute()
                        .left(px(4.))
                        .right(px(4.))
                        .top(px(5.5))
                        .h(px(1.))
                        .bg(t::T1),
                );
            BaseSlider::new(&self.state)
                .vertical()
                .disabled(self.disabled)
                .flex_1()
                .w(px(24.))
                .child(
                    track.size_full().justify_center().child(
                        SliderIndicator::new(&self.state)
                            .relative()
                            .h_full()
                            .w(px(4.))
                            .rounded_full()
                            .bg(t::G)
                            .shadow(vec![shadow(t::LN, 0., 0., 1., false)])
                            .child(cap),
                    ),
                )
        } else {
            let (from, to) = if self.bipolar {
                (p.min(0.5), p.max(0.5))
            } else {
                (0., p)
            };
            let fill = div()
                .absolute()
                .h_full()
                .left(relative(from))
                .right(relative(1. - to))
                .bg(if self.disabled { t::T3 } else { t::AC })
                .map(|f| match (self.bipolar, p < 0.5) {
                    (false, _) => f.rounded_full(),
                    (true, true) => f.rounded_l_full(),
                    (true, false) => f.rounded_r_full(),
                });
            let ring = if dragging {
                Some(t::alpha(t::AC, 0.25))
            } else {
                None
            };
            let thumb = SliderThumb::new(&self.state)
                .axis(axis)
                .disabled(self.disabled)
                .absolute()
                .left(relative(p))
                .rounded_full()
                .bg(t::T1)
                .map(|th| {
                    if dragging {
                        th.size(px(12.)).top(px(-4.5)).ml(px(-6.))
                    } else {
                        th.size(px(10.)).top(px(-3.5)).ml(px(-5.))
                    }
                })
                .shadow(thumb_shadow(ring))
                .when(!dragging && !self.disabled, |th| {
                    th.group_hover(group.clone(), |s| {
                        s.size(px(12.))
                            .top(px(-4.5))
                            .ml(px(-6.))
                            .shadow(thumb_shadow(Some(t::alpha(t::ON_AC, 0.08))))
                    })
                });
            BaseSlider::new(&self.state)
                .disabled(self.disabled)
                .w_full()
                .h(px(14.))
                .child(
                    track.size_full().items_center().child(
                        SliderIndicator::new(&self.state)
                            .relative()
                            .w_full()
                            .h(px(3.))
                            .rounded_full()
                            .bg(t::R1)
                            .shadow(vec![shadow(t::alpha(t::ON_AC, 0.03), 0., 0., 1., true)])
                            .when(!self.disabled, |s| {
                                s.group_hover(group.clone(), |s| s.bg(t::LN))
                            })
                            .when(self.bipolar, |s| {
                                s.child(
                                    div()
                                        .absolute()
                                        .left(relative(0.5))
                                        .top(px(-4.5))
                                        .w(px(1.))
                                        .h(px(12.))
                                        .bg(t::T3),
                                )
                            })
                            .child(fill)
                            .child(thumb),
                    ),
                )
        };

        let header = (self.label.is_some() || self.readout.is_some()).then(|| {
            let (label, value) = if dragging {
                (t::T1, t::AC)
            } else {
                (t::T3, t::T3)
            };
            h_flex()
                .justify_between()
                .gap_2()
                .text_size(px(10.))
                .children(self.label.map(|l| {
                    div()
                        .text_color(label)
                        .when(!dragging, |d| {
                            d.group_hover(group.clone(), |s| s.text_color(t::T2))
                        })
                        .child(l)
                }))
                .children(self.readout.map(|v| {
                    div()
                        .font_family(t::mono())
                        .text_color(value)
                        .when(!dragging, |d| {
                            d.group_hover(group.clone(), |s| s.text_color(t::T2))
                        })
                        .child(v)
                }))
        });

        let mut outer = v_flex()
            .id(("flikk-slider-group", id))
            .group(group.clone())
            .gap(px(8.))
            .map(|o| {
                if self.vertical {
                    o.h_full().items_center()
                } else {
                    o.w_full()
                }
            })
            .when(self.disabled, |o| o.opacity(0.4));
        outer.style().refine(&self.style);
        outer.children(header).child(slider)
    }
}
