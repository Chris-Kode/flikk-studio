//! Text page: font and fill, stroke, background, shadow, placement and the
//! line rules captions are wrapped by.

use super::CaptionStylePanel;
use super::controls::{Field, Paint, label, pairs, section, wide};
use crate::ui::kit::{Button, Segmented, Select, Switch, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::memory::{CaptionPosition, CaptionStyle};

const POSITIONS: [CaptionPosition; 3] = [
    CaptionPosition::Top,
    CaptionPosition::Middle,
    CaptionPosition::Bottom,
];

/// A 22 px square toggle holding `face`.
fn toggle(id: &'static str, on: bool, face: Div) -> Button {
    Button::new(id)
        .small()
        .selected(on)
        .w(px(22.))
        .px(px(0.))
        .child(face)
}

impl CaptionStylePanel {
    pub(super) fn text_page(&mut self, style: &CaptionStyle, cx: &mut Context<Self>) -> Div {
        let (bold, italic, upper) = (style.bold, style.italic, style.uppercase);
        let font = section("Font", true, Empty)
            .child(
                h_flex()
                    .h(px(26.))
                    .gap(px(4.))
                    .child(
                        div().flex_1().min_w_0().child(
                            // A font that isn't installed still names itself.
                            Select::new(&self.fonts)
                                .small()
                                .placeholder(style.font.clone()),
                        ),
                    )
                    .child(
                        toggle(
                            "caption-bold",
                            bold,
                            div().font_weight(FontWeight::SEMIBOLD).child("B"),
                        )
                        .tooltip("Bold")
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.edit(|s| s.bold = !bold, cx)),
                        ),
                    )
                    .child(
                        toggle("caption-italic", italic, div().italic().child("I"))
                            .tooltip("Italic")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.edit(|s| s.italic = !italic, cx)
                            })),
                    )
                    .child(
                        toggle(
                            "caption-case",
                            upper,
                            div()
                                .text_size(px(10.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("AA"),
                        )
                        .tooltip("Uppercase")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.edit(|s| s.uppercase = !upper, cx)
                        })),
                    ),
            )
            .child(pairs([
                self.number(Field::Size, cx),
                self.number(Field::Spacing, cx),
            ]))
            .child(wide(label("Color"), self.color(Paint::Text)));

        let stroke = section("Stroke", false, Empty).child(pairs([
            (label("Color"), self.color(Paint::Stroke)),
            self.number(Field::Outline, cx),
        ]));

        let boxed = style.background.enabled;
        let background = section(
            "Background",
            false,
            Switch::new("caption-background")
                .checked(boxed)
                .on_click(cx.listener(|this, on: &bool, _, cx| {
                    let on = *on;
                    this.edit(|s| s.background.enabled = on, cx)
                })),
        )
        .when(boxed, |d| {
            d.child(pairs([
                (label("Color"), self.color(Paint::Background)),
                self.number(Field::Padding, cx),
            ]))
        });

        let shadow = section("Shadow", false, Empty)
            .child(pairs([
                (label("Color"), self.color(Paint::Shadow)),
                self.number(Field::Shadow, cx),
            ]))
            .when(style.shadow > 0., |d| {
                d.child(pairs([self.number(Field::Angle, cx)]))
            });

        let position = POSITIONS
            .iter()
            .position(|p| *p == style.position)
            .unwrap_or(2);
        let place = section("Position", false, Empty)
            .child(
                h_flex().h(px(26.)).child(
                    Segmented::new("caption-position")
                        .compact()
                        .fill()
                        .items(["Top", "Middle", "Bottom"])
                        .selected(position)
                        .on_click(cx.listener(|this, ix: &usize, _, cx| {
                            let p = POSITIONS[(*ix).min(2)];
                            this.edit(|s| s.position = p, cx);
                        })),
                ),
            )
            .child(pairs([
                self.number(Field::X, cx),
                self.number(Field::Y, cx),
            ]));

        let lines = section("Lines", false, Empty).child(pairs([
            self.number(Field::LineLength, cx),
            (
                label("Lines"),
                Segmented::new("caption-lines")
                    .compact()
                    .fill()
                    .items(["1", "2", "3"])
                    .selected((style.max_lines.clamp(1, 3) - 1) as usize)
                    .on_click(cx.listener(|this, ix: &usize, _, cx| {
                        let n = *ix as u32 + 1;
                        this.edit(|s| s.max_lines = n, cx);
                    }))
                    .into_any_element(),
            ),
        ]));

        v_flex()
            .child(font)
            .child(stroke)
            .child(background)
            .child(shadow)
            .child(place)
            .child(lines)
    }
}
