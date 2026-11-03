//! One numeric property as an inspector row: a label that scrubs, a number
//! field and the row's keyframe lane. The kit's [`NumberFieldState`] owns
//! the scrub and the typing; a scrub shows its value in the viewer
//! uncommitted and commits once, on release, as does Enter or blur.

use super::{InspectorPanel, label, row};
use crate::ui::kit::{NumberField, NumberFieldEvent, NumberFieldState, ScrubLabel, h_flex};
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

/// A value of a mask.
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

/// Range and display of a key. Fields work in display units
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
        };
        match self {
            Key::PosX => s("Position X", -100., 100., 0.1, 1, 100., 0.),
            Key::PosY => s("Position Y", -100., 100., 0.1, 1, 100., 0.),
            Key::Scale => s("Scale", 10., 400., 1., 0, 100., 1.),
            Key::ScaleX => s("Stretch W", 0., 400., 1., 0, 100., 1.),
            Key::ScaleY => s("Stretch H", 0., 400., 1., 0, 100., 1.),
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
                [
                    "Top left X",
                    "Top left Y",
                    "Top right X",
                    "Top right Y",
                    "Bottom right X",
                    "Bottom right Y",
                    "Bottom left X",
                    "Bottom left Y",
                ][i.min(7)],
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
            Key::Lift(_) => s("Lift", -50., 50., 0.5, 1, 100., 0.),
            Key::GammaRgb(_) => s("Gamma", 50., 200., 0.5, 1, 100., 1.),
            Key::Gain(_) => s("Gain", 0., 200., 0.5, 1, 100., 1.),
            Key::FadeIn => s("Fade in", 0., 10., 0.05, 2, 1., 0.),
            Key::FadeOut => s("Fade out", 0., 10., 0.05, 2, 1., 0.),
            Key::Volume(_) => s("Volume", -60., 12., 0.1, 1, 1., 0.),
            Key::Pan(_) => s("Pan", -100., 100., 1., 0, 100., 0.),
            Key::DuckReduction => s("Reduction", -40., 0., 0.5, 1, 1., -12.),
            Key::DuckAttack => s("Attack", 0., 2000., 10., 0, 1000., 0.2),
            Key::DuckRelease => s("Release", 0., 4000., 10., 0, 1000., 0.6),
            Key::Smoothness => s("Amount", 0., 100., 1., 0, 100., 0.5),
        }
    }

    /// The unit shown after the value.
    fn unit(self) -> Option<&'static str> {
        Some(match self {
            Key::Scale | Key::ScaleX | Key::ScaleY | Key::Opacity | Key::Crop(_) => "%",
            Key::Rotation | Key::SkewX | Key::SkewY => "°",
            Key::Mask(_, MaskField::Rotation) => "°",
            Key::Mask(_, MaskField::Feather | MaskField::Expansion) => return None,
            Key::Mask(..) | Key::Smoothness => "%",
            Key::TransitionDuration | Key::FadeIn | Key::FadeOut => "s",
            Key::Volume(_) | Key::DuckReduction => "dB",
            Key::DuckAttack | Key::DuckRelease => "ms",
            _ => return None,
        })
    }
}

/// The field behind one key, kept across renders.
pub struct Param {
    field: Entity<NumberFieldState>,
    factor: f32,
    _subscription: Subscription,
}

impl Param {
    fn new(key: Key, value: f32, window: &mut Window, cx: &mut Context<InspectorPanel>) -> Self {
        let spec = key.spec();
        let factor = spec.factor;
        let field = cx.new(|cx| {
            NumberFieldState::new(window, cx)
                .min(spec.min as f64)
                .max(spec.max as f64)
                .step(spec.step as f64)
                .decimals(spec.decimals)
                .default_value((value * factor) as f64)
        });
        let _subscription = cx.subscribe(
            &field,
            move |this, _, ev: &NumberFieldEvent, cx| match *ev {
                NumberFieldEvent::Change(v) => this.preview(key, v as f32 / factor, cx),
                NumberFieldEvent::Commit(v) => this.commit(key, v as f32 / factor, cx),
            },
        );
        Self {
            field,
            factor,
            _subscription,
        }
    }

    /// Follow the model; a scrub in progress keeps its value.
    fn sync(&self, value: f32, cx: &mut App) {
        let shown = (value * self.factor) as f64;
        self.field.update(cx, |f, cx| f.set_value(shown, cx));
    }
}

impl InspectorPanel {
    /// The field for `key`, created on first use and synced to `value`.
    fn param(&mut self, key: Key, value: f32, window: &mut Window, cx: &mut Context<Self>) {
        self.params
            .entry(key)
            .or_insert_with(|| Param::new(key, value, window, cx))
            .sync(value, cx);
    }

    /// Double-click a label: back to the key's default.
    fn reset(key: Key, cx: &Context<Self>) -> impl Fn(&mut Window, &mut App) + 'static {
        let panel = cx.entity().downgrade();
        let default = key.spec().default;
        move |_, cx| {
            let _ = panel.update(cx, |this, cx| this.commit(key, default, cx));
        }
    }

    /// `key` on its own row: its label scrubs it. `keyed` marks a keyframe
    /// at the playhead; `lane` is the keyframe navigation, if it has one.
    pub(super) fn param_row(
        &mut self,
        key: Key,
        value: f32,
        keyed: bool,
        lane: Option<AnyElement>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        self.param(key, value, window, cx);
        let field = &self.params[&key].field;
        let label = ScrubLabel::new(field, key.spec().label)
            .keyed(keyed)
            .on_reset(Self::reset(key, cx));
        let number = NumberField::new(field)
            .keyed(keyed)
            .when_some(key.unit(), |f, unit| f.unit(unit));
        row(label, number, lane)
    }

    /// Two keys side by side under one label, as X and Y; each field's
    /// axis name scrubs it.
    pub(super) fn pair_row(
        &mut self,
        title: &'static str,
        keys: [(Key, f32, &'static str); 2],
        keyed: bool,
        lane: Option<AnyElement>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        for (key, value, _) in keys {
            self.param(key, value, window, cx);
        }
        let fields = keys.map(|(key, _, axis)| {
            let field = &self.params[&key].field;
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .child(
                    NumberField::new(field)
                        .keyed(keyed)
                        .pl(px(18.))
                        .when_some(key.unit(), |f, unit| f.unit(unit)),
                )
                .child(
                    // Over the field, so a drag on the axis scrubs rather
                    // than focusing the text.
                    div()
                        .absolute()
                        .left(px(6.))
                        .top_0()
                        .bottom_0()
                        .flex()
                        .items_center()
                        .occlude()
                        .child(
                            ScrubLabel::new(field, axis)
                                .keyed(keyed)
                                .on_reset(Self::reset(key, cx)),
                        ),
                )
        });
        let [x, y] = fields;
        row(label(title), h_flex().gap(px(4.)).child(x).child(y), lane)
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
