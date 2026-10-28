//! Text page: font and fill, stroke, background, shadow, placement and the
//! line rules captions are wrapped by.

use super::CaptionStylePanel;
use super::controls::{Field, Paint, number_row, row, swatch};
use crate::ui::inspector::{section, section_with};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::select::Select;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::memory::{CaptionPosition, CaptionStyle};

const POSITIONS: [CaptionPosition; 3] = [
    CaptionPosition::Top,
    CaptionPosition::Middle,
    CaptionPosition::Bottom,
];

impl CaptionStylePanel {
    pub(super) fn text_page(&mut self, style: &CaptionStyle, cx: &mut Context<Self>) -> Div {
        let (bold, italic, upper) = (style.bold, style.italic, style.uppercase);
        let toggle = |id: &'static str, on: bool| Button::new(id).ghost().small().toggled(on);
        let face = h_flex()
            .gap_0p5()
            .child(
                toggle("caption-bold", bold)
                    .child(div().font_weight(FontWeight::BOLD).child("B"))
                    .tooltip("Bold")
                    .on_click(cx.listener(move |this, _, _, cx| this.edit(|s| s.bold = !bold, cx))),
            )
            .child(
                toggle("caption-italic", italic)
                    .child(div().italic().child("I"))
                    .tooltip("Italic")
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.edit(|s| s.italic = !italic, cx)),
                    ),
            )
            .child(
                toggle("caption-case", upper)
                    .icon(IconName::CaseSensitive)
                    .tooltip("Uppercase")
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.edit(|s| s.uppercase = !upper, cx)),
                    ),
            );
        let font = section("Font", cx)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Select::new(&self.fonts).small()),
                    )
                    .child(face),
            )
            .child(number_row(self, Field::Size, cx))
            .child(row("Color", swatch(self, Paint::Text), cx))
            .child(number_row(self, Field::Spacing, cx));

        let stroke = section("Stroke", cx)
            .child(row("Color", swatch(self, Paint::Stroke), cx))
            .child(number_row(self, Field::Outline, cx));

        let boxed = style.background.enabled;
        let background = section_with(
            "Background",
            Switch::new("caption-background")
                .small()
                .checked(boxed)
                .on_click(cx.listener(|this, on: &bool, _, cx| {
                    let on = *on;
                    this.edit(|s| s.background.enabled = on, cx)
                })),
            cx,
        )
        .when(boxed, |d| {
            d.child(row("Color", swatch(self, Paint::Background), cx))
                .child(number_row(self, Field::Padding, cx))
        });

        let shadow = section("Shadow", cx)
            .child(row("Color", swatch(self, Paint::Shadow), cx))
            .child(number_row(self, Field::Shadow, cx))
            .when(style.shadow > 0., |d| {
                d.child(number_row(self, Field::Angle, cx))
            });

        let position = POSITIONS
            .iter()
            .position(|p| *p == style.position)
            .unwrap_or(2);
        let place = section("Position", cx)
            .child(
                TabBar::new("caption-position")
                    .segmented()
                    .xsmall()
                    .w_full()
                    .selected_index(position)
                    .child(Tab::new().label("Top").flex_1())
                    .child(Tab::new().label("Middle").flex_1())
                    .child(Tab::new().label("Bottom").flex_1())
                    .on_click(cx.listener(|this, ix: &usize, _, cx| {
                        let p = POSITIONS[(*ix).min(2)];
                        this.edit(|s| s.position = p, cx);
                    })),
            )
            .child(number_row(self, Field::X, cx))
            .child(number_row(self, Field::Y, cx));

        let lines = section("Lines", cx)
            .child(number_row(self, Field::LineLength, cx))
            .child(row(
                "Lines",
                TabBar::new("caption-lines")
                    .segmented()
                    .xsmall()
                    .w_full()
                    .selected_index((style.max_lines.clamp(1, 3) - 1) as usize)
                    .children((1..=3).map(|n| Tab::new().label(n.to_string()).flex_1()))
                    .on_click(cx.listener(|this, ix: &usize, _, cx| {
                        let n = *ix as u32 + 1;
                        this.edit(|s| s.max_lines = n, cx);
                    })),
                cx,
            ));

        v_flex()
            .gap_4()
            .child(font)
            .child(stroke)
            .child(background)
            .child(shadow)
            .child(place)
            .child(lines)
    }
}
