//! One numeric property: a slider plus an optional number field. Values are
//! committed once, on slider release or Enter/blur, never per pixel of drag;
//! while dragging, the viewer shows the value uncommitted.

use super::InspectorPanel;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::TrackId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    PosX,
    PosY,
    Scale,
    ScaleX,
    ScaleY,
    Rotation,
    SkewX,
    SkewY,
    AnchorX,
    AnchorY,
    Opacity,
    /// Crop edge: left, top, right, bottom.
    Crop(usize),
    /// Corner pin, corner by corner (top-left, top-right, bottom-right,
    /// bottom-left), x then y: `corner × 2 + axis`.
    Pin(usize),
    /// A value of the clip's n-th mask.
    Mask(usize, MaskField),
    /// The selected transition's length in seconds.
    TransitionDuration,
    Exposure,
    Contrast,
    Saturation,
    Gamma,
    Temperature,
    Tint,
    Lift(usize),
    GammaRgb(usize),
    Gain(usize),
    FadeIn,
    FadeOut,
    Volume(TrackId),
    Pan(TrackId),
    DuckReduction,
    DuckAttack,
    DuckRelease,
    /// The selected clip's stabilization smoothness.
    Smoothness,
}

/// A slider of a mask.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MaskField {
    Feather,
    Expansion,
    Opacity,
    /// Rectangles and ellipses.
    Rotation,
    /// Rectangles.
    Roundness,
    /// People: how much the edge follows the picture's.
    Edge,
    /// Depth masks: the range kept, and its fade.
    Near,
    Far,
    Softness,
}

impl MaskField {
    pub fn get(self, state: &koo_core::mask::MaskState) -> f32 {
        use koo_core::mask::MaskShape;
        match (self, &state.shape) {
            (Self::Feather, _) => state.feather,
            (Self::Expansion, _) => state.expansion,
            (Self::Opacity, _) => state.opacity,
            (
                Self::Rotation,
                MaskShape::Rectangle { rotation_deg, .. } | MaskShape::Ellipse { rotation_deg, .. },
            ) => *rotation_deg,
            (Self::Roundness, MaskShape::Rectangle { roundness, .. }) => *roundness,
            (Self::Edge, MaskShape::Person { edge }) => *edge,
            (Self::Near, MaskShape::Depth { near, .. }) => *near,
            (Self::Far, MaskShape::Depth { far, .. }) => *far,
            (Self::Softness, MaskShape::Depth { softness, .. }) => *softness,
            _ => 0.0,
        }
    }

    pub fn set(self, state: &mut koo_core::mask::MaskState, v: f32) {
        use koo_core::mask::MaskShape;
        match (self, &mut state.shape) {
            (Self::Feather, _) => state.feather = v.max(0.0),
            (Self::Expansion, _) => state.expansion = v,
            (Self::Opacity, _) => state.opacity = v.clamp(0.0, 1.0),
            (
                Self::Rotation,
                MaskShape::Rectangle { rotation_deg, .. } | MaskShape::Ellipse { rotation_deg, .. },
            ) => *rotation_deg = v,
            (Self::Roundness, MaskShape::Rectangle { roundness, .. }) => {
                *roundness = v.clamp(0.0, 1.0)
            }
            (Self::Edge, MaskShape::Person { edge }) => *edge = v.clamp(0.0, 1.0),
            (Self::Near, MaskShape::Depth { near, .. }) => *near = v.clamp(0.0, 1.0),
            (Self::Far, MaskShape::Depth { far, .. }) => *far = v.clamp(0.0, 1.0),
            (Self::Softness, MaskShape::Depth { softness, .. }) => *softness = v.clamp(0.0, 1.0),
            _ => {}
        }
    }
}

/// Range and display of a key. Sliders and fields work in display units
/// (`model × factor`).
pub struct Spec {
    pub label: &'static str,
    pub min: f32,
    pub max: f32,
    pub step: f32,
    pub decimals: usize,
    pub factor: f32,
    /// Model units; double-click the label to reset.
    pub default: f32,
    pub field: bool,
}

impl Key {
    pub fn spec(self) -> Spec {
        let s = |label, min, max, step, decimals, factor, default| Spec {
            label,
            min,
            max,
            step,
            decimals,
            factor,
            default,
            field: true,
        };
        match self {
            Key::PosX => s("X", -100., 100., 0.1, 1, 100., 0.),
            Key::PosY => s("Y", -100., 100., 0.1, 1, 100., 0.),
            Key::Scale => s("Scale", 10., 400., 1., 0, 100., 1.),
            Key::ScaleX => s("Scale X", 0., 400., 1., 0, 100., 1.),
            Key::ScaleY => s("Scale Y", 0., 400., 1., 0, 100., 1.),
            Key::Rotation => s("Rotation", -180., 180., 0.5, 1, 1., 0.),
            Key::SkewX => s("Skew X", -80., 80., 0.5, 1, 1., 0.),
            Key::SkewY => s("Skew Y", -80., 80., 0.5, 1, 1., 0.),
            Key::AnchorX => s("Anchor X", -100., 100., 0.5, 1, 100., 0.),
            Key::AnchorY => s("Anchor Y", -100., 100., 0.5, 1, 100., 0.),
            Key::Opacity => s("Opacity", 0., 100., 1., 0, 100., 1.),
            Key::Crop(i) => s(
                ["Left", "Top", "Right", "Bottom"][i.min(3)],
                0.,
                100.,
                0.5,
                1,
                100.,
                0.,
            ),
            Key::Pin(i) => s(
                ["↖ X", "↖ Y", "↗ X", "↗ Y", "↘ X", "↘ Y", "↙ X", "↙ Y"][i.min(7)],
                -100.,
                100.,
                0.5,
                1,
                100.,
                0.,
            ),
            Key::Mask(_, field) => match field {
                MaskField::Feather => s("Feather", 0., 50., 0.1, 1, 100., 0.),
                MaskField::Expansion => s("Expand", -50., 50., 0.1, 1, 100., 0.),
                MaskField::Opacity => s("Opacity", 0., 100., 1., 0, 100., 1.),
                MaskField::Rotation => s("Rotation", -180., 180., 0.5, 1, 1., 0.),
                MaskField::Roundness => s("Round", 0., 100., 1., 0, 100., 0.),
                MaskField::Edge => s("Edge", 0., 100., 1., 0, 100., 0.5),
                MaskField::Near => s("Near", 0., 100., 1., 0, 100., 0.5),
                MaskField::Far => s("Far", 0., 100., 1., 0, 100., 1.),
                MaskField::Softness => s("Soft", 0., 100., 1., 0, 100., 0.05),
            },
            Key::TransitionDuration => s("Duration", 0.1, 5., 0.05, 2, 1., 1.),
            Key::Exposure => s("Exposure", -4., 4., 0.01, 2, 1., 0.),
            Key::Contrast => s("Contrast", 0., 200., 1., 0, 100., 1.),
            Key::Saturation => s("Saturation", 0., 200., 1., 0, 100., 1.),
            Key::Gamma => s("Gamma", 0.2, 3., 0.01, 2, 1., 1.),
            Key::Temperature => s("Temp", -100., 100., 1., 0, 100., 0.),
            Key::Tint => s("Tint", -100., 100., 1., 0, 100., 0.),
            Key::Lift(_) => Spec {
                field: false,
                ..s("Lift", -50., 50., 0.5, 1, 100., 0.)
            },
            Key::GammaRgb(_) => Spec {
                field: false,
                ..s("Gamma", 50., 200., 0.5, 1, 100., 1.)
            },
            Key::Gain(_) => Spec {
                field: false,
                ..s("Gain", 0., 200., 0.5, 1, 100., 1.)
            },
            Key::FadeIn => s("Fade in", 0., 10., 0.05, 2, 1., 0.),
            Key::FadeOut => s("Fade out", 0., 10., 0.05, 2, 1., 0.),
            Key::Volume(_) => Spec {
                field: false,
                ..s("Volume", -60., 12., 0.1, 1, 1., 0.)
            },
            Key::Pan(_) => Spec {
                field: false,
                ..s("Pan", -100., 100., 1., 0, 100., 0.)
            },
            Key::DuckReduction => s("Reduction", -40., 0., 0.5, 1, 1., -12.),
            Key::DuckAttack => s("Attack", 0., 2000., 10., 0, 1000., 0.2),
            Key::DuckRelease => s("Release", 0., 4000., 10., 0, 1000., 0.6),
            Key::Smoothness => s("Amount", 0., 100., 1., 0, 100., 0.5),
        }
    }
}

pub struct Param {
    key: Key,
    slider: Entity<SliderState>,
    field: Option<Entity<InputState>>,
    /// Last model value shown.
    value: f32,
    dragging: bool,
    /// The field holds an uncommitted edit.
    typing: bool,
    _subscriptions: Vec<Subscription>,
}

impl Param {
    pub fn new(
        key: Key,
        value: f32,
        window: &mut Window,
        cx: &mut Context<InspectorPanel>,
    ) -> Self {
        let spec = key.spec();
        let slider = cx.new(|_| {
            SliderState::new()
                .min(spec.min)
                .max(spec.max)
                .step(spec.step)
                .default_value(value * spec.factor)
        });
        let mut subs = vec![cx.subscribe_in(
            &slider,
            window,
            move |this, _, ev: &SliderEvent, window, cx| match ev {
                SliderEvent::Change(v) => {
                    if let Some(p) = this.params.get_mut(&key) {
                        p.dragging = true;
                        p.show_field(v.start(), window, cx);
                    }
                    this.preview(key, v.start() / spec.factor, cx);
                }
                SliderEvent::Release(v) => {
                    if let Some(p) = this.params.get_mut(&key) {
                        p.dragging = false;
                    }
                    this.commit(key, v.start() / spec.factor, cx);
                }
            },
        )];
        let field = spec.field.then(|| {
            let text = format_value(value * spec.factor, spec.decimals);
            let field = cx.new(|cx| InputState::new(window, cx).default_value(text));
            subs.push(cx.subscribe_in(
                &field,
                window,
                move |this, input, ev: &InputEvent, _, cx| {
                    if let Some(p) = this.params.get_mut(&key) {
                        p.typing = matches!(ev, InputEvent::Change);
                    }
                    if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                        let parsed = input.read(cx).value().trim().parse::<f32>();
                        let current = this.params.get(&key).map(|p| p.value);
                        if let (Ok(v), Some(current)) = (parsed, current) {
                            let v = (v / spec.factor)
                                .clamp(spec.min / spec.factor, spec.max / spec.factor);
                            if (v - current).abs() > f32::EPSILON {
                                this.commit(key, v, cx);
                            }
                        }
                    }
                },
            ));
            field
        });
        Self {
            key,
            slider,
            field,
            value,
            dragging: false,
            typing: false,
            _subscriptions: subs,
        }
    }

    fn show_field(&self, display: f32, window: &mut Window, cx: &mut App) {
        if let Some(field) = &self.field {
            let text = format_value(display, self.key.spec().decimals);
            field.update(cx, |f, cx| f.set_value(text, window, cx));
        }
    }

    /// Resync on the next render, whether the commit applied or was refused.
    pub fn invalidate(&mut self) {
        self.value = f32::NAN;
    }

    /// Follow the model, except while the user drags this slider or types
    /// in its field.
    pub fn sync(&mut self, value: f32, window: &mut Window, cx: &mut App) {
        if self.dragging || (value - self.value).abs() < 1e-6 {
            return;
        }
        self.value = value;
        let display = value * self.key.spec().factor;
        self.slider
            .update(cx, |s, cx| s.set_value(display, window, cx));
        if !self.typing {
            self.show_field(display, window, cx);
        }
    }

    /// Label, slider and field on one 28 px row.
    pub fn row(&self, cx: &mut Context<InspectorPanel>) -> Div {
        let spec = self.key.spec();
        let key = self.key;
        let muted = cx.theme().muted_foreground;
        h_flex()
            .h(px(28.))
            .gap_2()
            .child(
                div()
                    .id(SharedString::from(format!("label-{key:?}")))
                    .w(px(64.))
                    .flex_none()
                    .text_xs()
                    .text_color(muted)
                    .truncate()
                    .child(spec.label)
                    .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                        if ev.click_count() >= 2 {
                            this.commit(key, spec.default, cx);
                        }
                    })),
            )
            .child(self.slider(None))
            .when_some(self.field.as_ref(), |row, field| {
                row.child(
                    div()
                        .w(px(56.))
                        .flex_none()
                        .child(Input::new(field).xsmall()),
                )
            })
    }

    pub fn slider(&self, color: Option<Hsla>) -> Div {
        div().flex_1().min_w_0().child(
            Slider::new(&self.slider)
                .when_some(color, |s, c| s.bg(c))
                .w_full(),
        )
    }
}

pub fn format_value(v: f32, decimals: usize) -> String {
    let s = format!("{v:.decimals$}");
    // Avoid "-0".
    if s.trim_start_matches('-')
        .chars()
        .all(|c| c == '0' || c == '.')
    {
        s.trim_start_matches('-').to_owned()
    } else {
        s
    }
}
