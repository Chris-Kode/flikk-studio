//! Tool calls in the thread (PRD AG10, V7). Calls made in a row stack into
//! one card; a row expands to its arguments, transcript excerpts and the
//! collages the model saw or the pictures it made. A call that takes a
//! while shows a spinner until it ends.

use super::ChatPanel;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, Theme, h_flex, v_flex};
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
        let theme = cx.theme().clone();
        let card = v_flex()
            .id(("tools", ix))
            .w_full()
            .overflow_hidden()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border);
        if let [entry] = entries {
            return card
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
        card.child(
            row(&theme)
                .id("tools-head")
                .child(status(ok, running, &theme))
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
                        .text_color(theme.muted_foreground)
                        .font_family(theme.mono_font_family.clone())
                        .child(names),
                )
                .child(chevron(open, &theme))
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
                    .border_color(theme.border)
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
        let theme = cx.theme().clone();
        let open = self.tool_open.contains(&(ix, j));
        v_flex()
            .child(
                row(&theme)
                    .id(("tool", j))
                    .child(status(entry.ok, entry.running.is_some(), &theme))
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
                    .child(chevron(open, &theme))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.tool_open.remove(&(ix, j)) {
                            this.tool_open.insert((ix, j));
                        }
                        this.remeasure(cx);
                    })),
            )
            .when(open, |el| el.child(details(entry, j, &theme)))
    }
}

fn row(theme: &Theme) -> Div {
    h_flex()
        .h(px(30.))
        .px_2p5()
        .gap_2()
        .text_xs()
        .cursor_pointer()
        .hover(|el| el.bg(theme.accent.opacity(0.5)))
}

fn status(ok: bool, running: bool, theme: &Theme) -> AnyElement {
    if running {
        return Spinner::new()
            .with_size(px(14.))
            .color(theme.muted_foreground)
            .into_any_element();
    }
    Icon::new(if ok {
        IconName::CircleCheck
    } else {
        IconName::CircleX
    })
    .size_3p5()
    .text_color(if ok { theme.green } else { theme.red })
    .into_any_element()
}

fn chevron(open: bool, theme: &Theme) -> Icon {
    Icon::new(if open {
        IconName::ChevronDown
    } else {
        IconName::ChevronRight
    })
    .size_3()
    .text_color(theme.muted_foreground)
}

fn details(entry: &ActivityEntry, j: usize, theme: &Theme) -> impl IntoElement {
    let args = serde_json::to_string_pretty(&entry.args).unwrap_or_default();
    v_flex()
        .px_2p5()
        .pb_2p5()
        .gap_2()
        .text_xs()
        .when(!entry.args.is_null(), |el| {
            el.child(
                div()
                    .id(("tool-args", j))
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
