//! The style's numbers and colours, and the rows they sit in. A number is a
//! scrub label and a field, like the inspector's: scrubbing shows the value
//! in the viewer, release or Enter saves it, double-clicking the label
//! resets it.

use super::CaptionStylePanel;
use crate::ui::kit::theme as t;
use crate::ui::kit::{
    ColorField, NumberField, NumberFieldEvent, NumberFieldState, ScrubLabel, h_flex, v_flex,
};
use flikk_core::memory::{CaptionStyle, Color};
use gpui_kit::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Field {
    Size,
    Spacing,
    Outline,
    Shadow,
    Angle,
    Padding,
    X,
    Y,
    LineLength,
    Intro,
    Outro,
}

pub struct Spec {
    pub label: &'static str,
    pub min: f32,
    pub max: f32,
    pub step: f32,
    pub decimals: usize,
    pub unit: Option<&'static str>,
}

impl Field {
    pub const ALL: [Field; 11] = [
        Self::Size,
        Self::Spacing,
        Self::Outline,
        Self::Shadow,
        Self::Angle,
        Self::Padding,
        Self::X,
        Self::Y,
        Self::LineLength,
        Self::Intro,
        Self::Outro,
    ];

    pub fn spec(self) -> Spec {
        let s = |label, min, max, step, decimals, unit| Spec {
            label,
            min,
            max,
            step,
            decimals,
            unit,
        };
        match self {
            Self::Size => s("Size", 12., 160., 1., 0, Some("px")),
            Self::Spacing => s("Spacing", -5., 30., 0.5, 1, None),
            Self::Outline => s("Width", 0., 12., 0.5, 1, Some("px")),
            Self::Shadow => s("Distance", 0., 20., 0.5, 1, Some("px")),
            Self::Angle => s("Angle", 0., 360., 5., 0, Some("°")),
            Self::Padding => s("Padding", 0., 40., 1., 0, Some("px")),
            Self::X => s("X", -50., 50., 0.5, 1, Some("%")),
            Self::Y => s("Y", -50., 50., 0.5, 1, Some("%")),
            Self::LineLength => s("Length", 12., 60., 1., 0, Some("ch")),
            Self::Intro | Self::Outro => s("Duration", 0.05, 2., 0.05, 2, Some("s")),
        }
    }

    pub fn get(self, s: &CaptionStyle) -> f32 {
        match self {
            Self::Size => s.size,
            Self::Spacing => s.spacing,
            Self::Outline => s.outline,
            Self::Shadow => s.shadow,
            Self::Angle => s.shadow_angle,
            Self::Padding => s.background.padding,
            Self::X => s.offset_x,
            Self::Y => s.offset_y,
            Self::LineLength => s.max_chars_per_line as f32,
            Self::Intro => s.intro.duration,
            Self::Outro => s.outro.duration,
        }
    }

    pub fn set(self, s: &mut CaptionStyle, v: f32) {
        let spec = self.spec();
        let v = v.clamp(spec.min, spec.max);
        match self {
            Self::Size => s.size = v,
            Self::Spacing => s.spacing = v,
            Self::Outline => s.outline = v,
            Self::Shadow => s.shadow = v,
            Self::Angle => s.shadow_angle = v,
            Self::Padding => s.background.padding = v,
            Self::X => s.offset_x = v,
            Self::Y => s.offset_y = v,
            Self::LineLength => s.max_chars_per_line = v.round() as u32,
            Self::Intro => s.intro.duration = v,
            Self::Outro => s.outro.duration = v,
        }
    }
}

/// `field`'s scrub field, showing `value`: a scrub previews, a commit saves.
pub fn number(
    field: Field,
    value: f32,
    window: &mut Window,
    cx: &mut Context<CaptionStylePanel>,
) -> (Entity<NumberFieldState>, Subscription) {
    let spec = field.spec();
    let state = cx.new(|cx| {
        NumberFieldState::new(window, cx)
            .min(spec.min.into())
            .max(spec.max.into())
            .step(spec.step.into())
            .decimals(spec.decimals)
            .default_value(value.into())
    });
    let sub = cx.subscribe(
        &state,
        move |this, _, ev: &NumberFieldEvent, cx| match *ev {
            NumberFieldEvent::Change(v) => this.show(|s| field.set(s, v as f32), cx),
            NumberFieldEvent::Commit(v) => this.edit(|s| field.set(s, v as f32), cx),
        },
    );
    (state, sub)
}

/// The muted label of a row.
pub fn label(text: &'static str) -> AnyElement {
    div()
        .min_w_0()
        .truncate()
        .text_size(px(11.))
        .text_color(t::T2)
        .child(text)
        .into_any_element()
}

/// A group: its 26 px title with `actions` on the right, then its rows.
/// Groups after the first are ruled off.
pub fn section(title: &'static str, first: bool, actions: impl IntoElement) -> Div {
    let group = v_flex().flex_none().child(
        h_flex()
            .h(px(26.))
            .gap(px(6.))
            .text_size(px(11.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(t::T2)
            .child(div().flex_1().child(title))
            .child(actions),
    );
    if first {
        group
    } else {
        group.mt(px(4.)).pt(px(2.)).border_t_1().border_color(t::LN)
    }
}

/// One or two label and control pairs on a 26 px row. A lone pair keeps to
/// the left half, so its field lines up with the rows around it.
pub fn pairs(pairs: impl IntoIterator<Item = (AnyElement, AnyElement)>) -> Div {
    let mut row = h_flex().h(px(26.)).gap(px(8.));
    let mut n = 0;
    for (label, control) in pairs {
        row = row
            .child(cell(label))
            .child(div().flex_1().min_w_0().child(control));
        n += 1;
    }
    if n == 1 {
        row = row
            .child(cell(Empty.into_any_element()))
            .child(div().flex_1());
    }
    row
}

/// A label and a control spanning the rest of the row.
pub fn wide(label: AnyElement, control: impl IntoElement) -> Div {
    h_flex()
        .h(px(26.))
        .gap(px(8.))
        .child(cell(label))
        .child(div().flex_1().min_w_0().child(control))
}

fn cell(label: AnyElement) -> Div {
    div().w(px(52.)).flex_none().min_w_0().child(label)
}

impl CaptionStylePanel {
    /// `field`'s scrub label and field.
    pub(super) fn number(&self, field: Field, cx: &mut Context<Self>) -> (AnyElement, AnyElement) {
        let Some(state) = self.numbers.get(&field) else {
            return (Empty.into_any_element(), Empty.into_any_element());
        };
        let spec = field.spec();
        let reset = field.get(&CaptionStyle::default());
        let panel = cx.entity().downgrade();
        let label = ScrubLabel::new(state, spec.label)
            .on_reset(move |_, cx| {
                let _ = panel.update(cx, |this, cx| this.edit(|s| field.set(s, reset), cx));
            })
            .into_any_element();
        let mut value = NumberField::new(state);
        if let Some(unit) = spec.unit {
            value = value.unit(unit);
        }
        (label, value.into_any_element())
    }

    /// `paint`'s colour field.
    pub(super) fn color(&self, paint: Paint) -> AnyElement {
        match self.colors.get(&paint) {
            Some((picker, _)) => {
                ColorField::new(SharedString::from(format!("caption-{paint:?}")), picker)
                    .into_any_element()
            }
            None => Empty.into_any_element(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Paint {
    Text,
    Stroke,
    Background,
    Shadow,
    Highlight,
}

impl Paint {
    pub const ALL: [Paint; 5] = [
        Self::Text,
        Self::Stroke,
        Self::Background,
        Self::Shadow,
        Self::Highlight,
    ];

    pub fn get(self, s: &CaptionStyle) -> Color {
        match self {
            Self::Text => s.color,
            Self::Stroke => s.outline_color,
            Self::Background => s.background.color,
            Self::Shadow => s.shadow_color,
            Self::Highlight => s.highlight_color,
        }
    }

    pub fn set(self, s: &mut CaptionStyle, c: Color) {
        match self {
            Self::Text => s.color = c,
            Self::Stroke => s.outline_color = c,
            Self::Background => s.background.color = c,
            Self::Shadow => s.shadow_color = c,
            Self::Highlight => s.highlight_color = c,
        }
    }
}

pub fn color_of(c: Hsla) -> Color {
    let rgb = c.to_rgb();
    let byte = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    Color {
        r: byte(rgb.r),
        g: byte(rgb.g),
        b: byte(rgb.b),
        a: byte(rgb.a),
    }
}
