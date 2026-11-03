//! The colour field: a 22 px field with a swatch and the hex value. The
//! swatch opens hue, saturation, lightness and alpha sliders; Enter in the
//! field takes a typed hex. GPUI Base's [`ColorPickerState`] keeps the
//! colour, the hex text and the sliders in step and emits each change.

use super::{button::Button, input::Input, popover::Popover, slider::Slider, theme as t};
use gpui_kit::base::{ColorPickerState, v_flex};
use gpui_kit::*;

#[derive(IntoElement)]
pub struct ColorField {
    id: ElementId,
    state: Entity<ColorPickerState>,
}

impl ColorField {
    pub fn new(id: impl Into<ElementId>, state: &Entity<ColorPickerState>) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
        }
    }
}

impl RenderOnce for ColorField {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        self.state
            .update(cx, |s, cx| s.sync_pending_value(window, cx));
        let s = self.state.read(cx);
        let color = s.displayed_color().unwrap_or(hsla(0., 0., 1., 1.));
        let (hex, sliders) = (s.hex_input().clone(), s.sliders().clone());
        // As narrow as the chip, so a half-row field still fits `#RRGGBB`.
        let swatch = Button::new(self.id.clone())
            .ghost()
            .xsmall()
            .w(px(14.))
            .child(
                div()
                    .flex_none()
                    .size(px(12.))
                    .rounded(px(3.))
                    .border_1()
                    .border_color(t::alpha(t::ON_AC, 0.2))
                    .bg(color),
            );
        let picker = Popover::new(self.id)
            .trigger(swatch)
            .content(move |_, _, _| {
                v_flex()
                    .w(px(200.))
                    .p(px(10.))
                    .gap(px(10.))
                    .child(Slider::new(sliders.hue()).label("Hue"))
                    .child(Slider::new(sliders.saturation()).label("Saturation"))
                    .child(Slider::new(sliders.lightness()).label("Lightness"))
                    .child(Slider::new(sliders.alpha()).label("Alpha"))
            });
        Input::new(&hex)
            .small()
            .pl(px(1.))
            .font_family(t::mono())
            .prefix(picker)
    }
}
