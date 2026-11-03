//! Tool calls in the thread (PRD AG10, V7). Calls made in a row stack into
//! one card; a row expands to its arguments, transcript excerpts and the
//! collages the model saw or the pictures it made. A call that takes a
//! while shows a spinner until it ends.

use super::ChatPanel;
use crate::ui::kit::{Icon, IconName, ScrollableElement as _, Spinner, h_flex, theme as t, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::agent::ActivityEntry;

impl ChatPanel {
    /// The stack of calls in message `ix`. More than one call collapses
    /// behind a count until opened.
    pub(super) fn render_tools(
        &mut self,
        ix: usize,
        entries: &[ActivityEntry],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let card = v_flex()
            .id(("tools", ix))
            .w_full()
            .overflow_hidden()
            .rounded(t::RADIUS)
            .border_1()
            .bg(t::P);
        if let [entry] = entries {
            return card
                .border_color(if failed(entry) {
                    t::alpha(t::ERR, 0.25)
                } else {
                    t::LN
                })
                .child(self.render_tool(ix, 0, entry, cx))
                .into_any_element();
        }
        let open = self.tools_open.contains(&ix);
        let ok = entries.iter().all(|e| e.ok);
        let running = entries.iter().any(|e| e.running.is_some());
        let names = entries
            .iter()
            .map(|e| e.tool.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        card.border_color(t::LN)
            .child(
                row()
                    .id("tools-head")
                    .when(open, |el| el.bg(t::HV))
                    .child(status(ok, running))
                    .child(
                        div()
                            .flex_none()
                            .child(format!("{} tool calls", entries.len())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(t::mono())
                            .text_color(t::T2)
                            .child(names),
                    )
                    .child(chevron(open))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.tools_open.remove(&ix) {
                            this.tools_open.insert(ix);
                        }
                        this.remeasure(cx);
                    })),
            )
            .when(open, |el| {
                el.children(entries.iter().enumerate().map(|(j, entry)| {
                    div()
                        .border_t_1()
                        .border_color(t::LN)
                        .child(self.render_tool(ix, j, entry, cx))
                }))
            })
            .into_any_element()
    }

    fn render_tool(
        &mut self,
        ix: usize,
        j: usize,
        entry: &ActivityEntry,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let open = self.tool_open.contains(&(ix, j));
        v_flex()
            .child(
                row()
                    .id(("tool", j))
                    .child(status(entry.ok, entry.running.is_some()))
                    .child(
                        div()
                            .flex_none()
                            .font_family(t::mono())
                            .child(entry.tool.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(if failed(entry) { t::ERR } else { t::T2 })
                            .child(entry.summary.clone()),
                    )
                    .child(chevron(open))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.tool_open.remove(&(ix, j)) {
                            this.tool_open.insert((ix, j));
                        }
                        this.remeasure(cx);
                    })),
            )
            .when(open, |el| el.child(details(entry, j)))
    }
}

fn failed(entry: &ActivityEntry) -> bool {
    !entry.ok && entry.running.is_none()
}

fn row() -> Div {
    h_flex()
        .h(px(28.))
        .px(px(10.))
        .gap(px(8.))
        .text_size(px(11.))
        .cursor_pointer()
        .hover(|el| el.bg(t::HV))
}

fn status(ok: bool, running: bool) -> AnyElement {
    if running {
        return Spinner::new().size(px(12.)).color(t::T2).into_any_element();
    }
    let (icon, color) = if ok {
        (IconName::CircleCheck, t::OK)
    } else {
        (IconName::CircleX, t::ERR)
    };
    Icon::new(icon).color(color).into_any_element()
}

fn chevron(open: bool) -> Icon {
    Icon::new(if open {
        IconName::ChevronDown
    } else {
        IconName::ChevronRight
    })
    .size(px(12.))
    .color(t::T3)
}

fn details(entry: &ActivityEntry, j: usize) -> impl IntoElement {
    let args = serde_json::to_string_pretty(&entry.args).unwrap_or_default();
    v_flex()
        .px(px(10.))
        .pb(px(10.))
        .gap(px(8.))
        .text_size(px(11.))
        .line_height(px(16.))
        .when(!entry.args.is_null(), |el| {
            el.child(
                v_flex()
                    .id(("tool-args", j))
                    .max_h(px(180.))
                    .px(px(8.))
                    .py(px(7.))
                    .rounded(t::RADIUS_SM)
                    .border_1()
                    .border_color(t::LN)
                    .bg(t::G)
                    .font_family(t::mono())
                    .text_size(px(10.5))
                    .text_color(t::T2)
                    .children(args.lines().map(|l| div().child(l.to_string())))
                    .overflow_y_scrollbar(),
            )
        })
        .children(entry.excerpts.iter().map(|e| {
            div()
                .pl(px(8.))
                .border_l_2()
                .border_color(t::LN)
                .text_color(t::T2)
                .child(e.clone())
        }))
        .when(!entry.images.is_empty(), |el| {
            el.child(
                h_flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .children(entry.images.iter().map(|path| {
                        img(path.clone())
                            .h(px(64.))
                            .max_w_full()
                            .rounded(t::RADIUS_SM)
                            .border_1()
                            .border_color(t::LN)
                    })),
            )
        })
}
