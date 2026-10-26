//! Activity log (PRD AG10, V7): every tool call, newest first; a row expands
//! to its arguments, transcript excerpts and the collages the model saw.

use super::ChatPanel;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::agent::ActivityEntry;

impl ChatPanel {
    pub(super) fn render_activity(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let entries = self.state.read(cx).activity.clone();
        let theme = cx.theme().clone();
        if entries.is_empty() {
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child("No tool calls yet")
                .into_any_element();
        }
        let rows = entries.into_iter().enumerate().rev().map(|(ix, entry)| {
            let open = self.activity_open.contains(&ix);
            v_flex()
                .border_b_1()
                .border_color(theme.border)
                .child(
                    h_flex()
                        .id(("activity", ix))
                        .h(px(30.))
                        .px_3()
                        .gap_2()
                        .text_xs()
                        .cursor_pointer()
                        .hover(|el| el.bg(theme.accent.opacity(0.5)))
                        .child(
                            Icon::new(if entry.ok {
                                IconName::CircleCheck
                            } else {
                                IconName::CircleX
                            })
                            .size_3p5()
                            .text_color(if entry.ok {
                                theme.green
                            } else {
                                theme.red
                            }),
                        )
                        .child(
                            div()
                                .flex_none()
                                .font_family(theme.mono_font_family.clone())
                                .child(entry.tool.clone()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(theme.muted_foreground)
                                .child(entry.summary.clone()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_color(theme.muted_foreground)
                                .child(age(entry.at)),
                        )
                        .child(
                            Icon::new(if open {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .size_3()
                            .text_color(theme.muted_foreground),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.activity_open.remove(&ix) {
                                this.activity_open.insert(ix);
                            }
                            cx.notify();
                        })),
                )
                .when(open, |el| el.child(details(&entry, ix, &theme)))
        });
        v_flex()
            .id("activity-log")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .children(rows)
            .into_any_element()
    }
}

fn details(
    entry: &ActivityEntry,
    ix: usize,
    theme: &gpui_kit::component::Theme,
) -> impl IntoElement {
    let args = serde_json::to_string_pretty(&entry.args).unwrap_or_default();
    v_flex()
        .px_3()
        .pb_3()
        .gap_2()
        .text_xs()
        .when(!entry.args.is_null(), |el| {
            el.child(
                div()
                    .id(("activity-args", ix))
                    .p_2()
                    .max_h(px(180.))
                    .overflow_y_scroll()
                    .rounded(theme.radius)
                    .bg(theme.muted)
                    .font_family(theme.mono_font_family.clone())
                    .whitespace_normal()
                    .children(
                        args.lines()
                            .map(|l| div().child(l.to_string()))
                            .collect::<Vec<_>>(),
                    ),
            )
        })
        .children(entry.excerpts.iter().map(|e| {
            div()
                .pl_2()
                .border_l_2()
                .border_color(theme.border)
                .text_color(theme.muted_foreground)
                .child(e.clone())
        }))
        .when(!entry.images.is_empty(), |el| {
            el.child(
                h_flex()
                    .flex_wrap()
                    .gap_1p5()
                    .children(entry.images.iter().map(|path| {
                        img(path.clone())
                            .h(px(72.))
                            .max_w_full()
                            .rounded(theme.radius)
                            .border_1()
                            .border_color(theme.border)
                    })),
            )
        })
}

/// Age of the call: `now`, `4m`, `2h`, `3d`.
fn age(at: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(at);
    match (now - at).max(0) {
        s if s < 60 => "now".into(),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}
