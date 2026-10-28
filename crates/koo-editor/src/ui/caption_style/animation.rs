//! Animation page: how the spoken word stands out, and how captions enter
//! and leave.

use super::CaptionStylePanel;
use super::controls::{Field, Paint, number_row, row, swatch};
use crate::ui::inspector::section;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{Sizable as _, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::memory::{CaptionHighlight, CaptionMotion, CaptionStyle, MotionKind};

impl CaptionStylePanel {
    pub(super) fn animation_page(&mut self, style: &CaptionStyle, cx: &mut Context<Self>) -> Div {
        let mode = style.highlight;
        let highlight = section("Highlight", cx)
            .child(
                TabBar::new("caption-highlight")
                    .segmented()
                    .xsmall()
                    .w_full()
                    .selected_index(
                        CaptionHighlight::ALL
                            .iter()
                            .position(|h| *h == mode)
                            .unwrap_or(0),
                    )
                    .children(
                        CaptionHighlight::ALL
                            .iter()
                            .map(|h| Tab::new().label(h.label()).flex_1()),
                    )
                    .on_click(cx.listener(|this, ix: &usize, _, cx| {
                        let h = CaptionHighlight::ALL[(*ix).min(CaptionHighlight::ALL.len() - 1)];
                        this.edit(|s| s.highlight = h, cx);
                    })),
            )
            .when(
                matches!(
                    mode,
                    CaptionHighlight::Color | CaptionHighlight::Box | CaptionHighlight::Karaoke
                ),
                |d| d.child(row("Color", swatch(self, Paint::Highlight), cx)),
            )
            .when(mode == CaptionHighlight::Box, |d| {
                d.child(number_row(self, Field::Padding, cx))
            });
        v_flex()
            .gap_4()
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
        section(title, cx)
            .child(
                TabBar::new(SharedString::from(format!("caption-{title}")))
                    .segmented()
                    .xsmall()
                    .w_full()
                    .selected_index(MotionKind::ALL.iter().position(|k| *k == kind).unwrap_or(0))
                    .children(
                        MotionKind::ALL
                            .iter()
                            .map(|k| Tab::new().label(k.label()).flex_1()),
                    )
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
            )
            .when(kind != MotionKind::None, |d| {
                d.child(number_row(self, field, cx))
            })
    }
}
