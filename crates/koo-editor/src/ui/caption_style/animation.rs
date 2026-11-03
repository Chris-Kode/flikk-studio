//! Animation page: how the spoken word stands out, and how captions enter
//! and leave.

use super::CaptionStylePanel;
use super::controls::{Field, Paint, label, pairs, section};
use crate::ui::kit::{Segmented, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::memory::{CaptionHighlight, CaptionMotion, CaptionStyle, MotionKind};

impl CaptionStylePanel {
    pub(super) fn animation_page(&mut self, style: &CaptionStyle, cx: &mut Context<Self>) -> Div {
        let mode = style.highlight;
        let colored = matches!(
            mode,
            CaptionHighlight::Color | CaptionHighlight::Box | CaptionHighlight::Karaoke
        );
        let highlight = section("Highlight", true, Empty)
            .child(
                h_flex().h(px(26.)).child(
                    Segmented::new("caption-highlight")
                        .compact()
                        .fill()
                        .items(CaptionHighlight::ALL.iter().map(|h| h.label()))
                        .selected(
                            CaptionHighlight::ALL
                                .iter()
                                .position(|h| *h == mode)
                                .unwrap_or(0),
                        )
                        .on_click(cx.listener(|this, ix: &usize, _, cx| {
                            let h =
                                CaptionHighlight::ALL[(*ix).min(CaptionHighlight::ALL.len() - 1)];
                            this.edit(|s| s.highlight = h, cx);
                        })),
                ),
            )
            .when(colored, |d| {
                let color = (label("Color"), self.color(Paint::Highlight));
                d.child(if mode == CaptionHighlight::Box {
                    pairs([color, self.number(Field::Padding, cx)])
                } else {
                    pairs([color])
                })
            });
        v_flex()
            .child(highlight)
            .child(self.motion("In", &style.intro, Field::Intro, cx))
            .child(self.motion("Out", &style.outro, Field::Outro, cx))
    }

    fn motion(
        &mut self,
        title: &'static str,
        motion: &CaptionMotion,
        field: Field,
        cx: &mut Context<Self>,
    ) -> Div {
        let kind = motion.kind;
        section(title, false, Empty)
            .child(
                h_flex().h(px(26.)).child(
                    Segmented::new(SharedString::from(format!("caption-{title}")))
                        .compact()
                        .fill()
                        .items(MotionKind::ALL.iter().map(|k| k.label()))
                        .selected(MotionKind::ALL.iter().position(|k| *k == kind).unwrap_or(0))
                        .on_click(cx.listener(move |this, ix: &usize, _, cx| {
                            let k = MotionKind::ALL[(*ix).min(MotionKind::ALL.len() - 1)];
                            this.edit(
                                |s| match field {
                                    Field::Intro => s.intro.kind = k,
                                    _ => s.outro.kind = k,
                                },
                                cx,
                            );
                        })),
                ),
            )
            .when(kind != MotionKind::None, |d| {
                d.child(pairs([self.number(field, cx)]))
            })
    }
}
