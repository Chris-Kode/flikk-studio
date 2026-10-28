//! The style's numbers and colors. A number is a slider plus a field, like
//! the inspector's (`inspector::param`): dragging shows the value in the
//! viewer, release or Enter saves it, double-clicking the label resets it.

use super::CaptionStylePanel;
use crate::ui::inspector::format_value;
use gpui_kit::component::color_picker::{ColorPicker, ColorPickerState};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex};
use gpui_kit::*;
use koo_core::memory::{CaptionStyle, Color};

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
        let s = |label, min, max, step, decimals| Spec {
            label,
            min,
            max,
            step,
            decimals,
        };
        match self {
            Self::Size => s("Size", 12., 160., 1., 0),
            Self::Spacing => s("Spacing", -5., 30., 0.5, 1),
            Self::Outline => s("Width", 0., 12., 0.5, 1),
            Self::Shadow => s("Distance", 0., 20., 0.5, 1),
            Self::Angle => s("Angle", 0., 360., 5., 0),
            Self::Padding => s("Padding", 0., 40., 1., 0),
            Self::X => s("X", -50., 50., 0.5, 1),
            Self::Y => s("Y", -50., 50., 0.5, 1),
            Self::LineLength => s("Length", 12., 60., 1., 0),
            Self::Intro | Self::Outro => s("Duration", 0.05, 2., 0.05, 2),
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

pub struct Number {
    field: Field,
    slider: Entity<SliderState>,
    input: Entity<InputState>,
    /// Last value shown.
    value: f32,
    dragging: bool,
    /// The input holds an unsaved edit.
    typing: bool,
    _subscriptions: Vec<Subscription>,
}

impl Number {
    pub fn new(
        field: Field,
        value: f32,
        window: &mut Window,
        cx: &mut Context<CaptionStylePanel>,
    ) -> Self {
        let spec = field.spec();
        let slider = cx.new(|_| {
            SliderState::new()
                .min(spec.min)
                .max(spec.max)
                .step(spec.step)
                .default_value(value)
        });
        let input = cx.new(|cx| {
            InputState::new(window, cx).default_value(format_value(value, spec.decimals))
        });
        let _subscriptions = vec![
            cx.subscribe_in(
                &slider,
                window,
                move |this, _, ev: &SliderEvent, window, cx| match ev {
                    SliderEvent::Change(v) => {
                        let v = v.start();
                        if let Some(n) = this.numbers.get_mut(&field) {
                            n.dragging = true;
                            n.show(v, window, cx);
                        }
                        this.show(|s| field.set(s, v), cx);
                    }
                    SliderEvent::Release(v) => {
                        let v = v.start();
                        if let Some(n) = this.numbers.get_mut(&field) {
                            n.dragging = false;
                            n.value = f32::NAN;
                        }
                        this.edit(|s| field.set(s, v), cx);
                    }
                },
            ),
            cx.subscribe_in(
                &input,
                window,
                move |this, input, ev: &InputEvent, _, cx| {
                    if let Some(n) = this.numbers.get_mut(&field) {
                        n.typing = matches!(ev, InputEvent::Change);
                    }
                    if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur)
                        && let Ok(v) = input.read(cx).value().trim().parse::<f32>()
                    {
                        if let Some(n) = this.numbers.get_mut(&field) {
                            n.value = f32::NAN;
                        }
                        this.edit(|s| field.set(s, v), cx);
                    }
                },
            ),
        ];
        Self {
            field,
            slider,
            input,
            value,
            dragging: false,
            typing: false,
            _subscriptions,
        }
    }

    fn show(&self, v: f32, window: &mut Window, cx: &mut App) {
        let text = format_value(v, self.field.spec().decimals);
        self.input.update(cx, |f, cx| f.set_value(text, window, cx));
    }

    /// Follow the model, except while dragged or typed in.
    pub fn sync(&mut self, value: f32, window: &mut Window, cx: &mut App) {
        if self.dragging || value == self.value {
            return;
        }
        self.value = value;
        self.slider
            .update(cx, |s, cx| s.set_value(value, window, cx));
        if !self.typing {
            self.show(value, window, cx);
        }
    }
}

/// Label, slider and field on one row.
pub fn number_row(
    panel: &CaptionStylePanel,
    field: Field,
    cx: &mut Context<CaptionStylePanel>,
) -> AnyElement {
    let Some(n) = panel.numbers.get(&field) else {
        return div().into_any_element();
    };
    let reset = field.get(&CaptionStyle::default());
    h_flex()
        .h(px(28.))
        .gap_2()
        .child(
            label(field.spec().label, cx)
                .id(SharedString::from(format!("{field:?}")))
                .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                    if ev.click_count() >= 2 {
                        this.edit(|s| field.set(s, reset), cx);
                    }
                })),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(Slider::new(&n.slider).w_full()),
        )
        .child(
            div()
                .w(px(52.))
                .flex_none()
                .child(Input::new(&n.input).xsmall()),
        )
        .into_any_element()
}

/// The muted label column of a row.
pub fn label(text: &'static str, cx: &App) -> Div {
    div()
        .w(px(64.))
        .flex_none()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .truncate()
        .child(text)
}

/// A labelled row holding `control`.
pub fn row(text: &'static str, control: impl IntoElement, cx: &App) -> Div {
    h_flex()
        .min_h(px(28.))
        .gap_2()
        .child(label(text, cx))
        .child(div().flex_1().min_w_0().child(control))
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

/// The swatch that opens `paint`'s picker.
pub fn swatch(panel: &CaptionStylePanel, paint: Paint) -> AnyElement {
    let picker: Option<&Entity<ColorPickerState>> = panel.colors.get(&paint).map(|(p, _)| p);
    match picker {
        Some(p) => ColorPicker::new(p).small().into_any_element(),
        None => div().into_any_element(),
    }
}
