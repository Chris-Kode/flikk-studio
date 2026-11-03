//! Quick-command bar (PRD AG1): cmd/ctrl-K from anywhere opens a palette to
//! type a one-off request or start a recipe. Both go through `send_chat`.

use super::composer::Composer;
use super::recipes;
use crate::state::EditorState;
use crate::ui::kit::{DialogExt as _, Icon, IconName, Input, Kbd, h_flex, theme as t, v_flex};
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::cell::Cell;

actions!(
    koo_chat,
    [ToggleQuickCommand, QuickUp, QuickDown, QuickDismiss]
);

const CONTEXT: &str = "ChatQuick";

pub fn init(cx: &mut App) {
    // Same depth as the input's own bindings and registered later, so these
    // win over its cursor keys.
    let ctx = Some("ChatQuick > Input");
    cx.bind_keys([
        KeyBinding::new("cmd-k", ToggleQuickCommand, None),
        KeyBinding::new("ctrl-k", ToggleQuickCommand, None),
        KeyBinding::new("up", QuickUp, ctx),
        KeyBinding::new("down", QuickDown, ctx),
        KeyBinding::new("escape", QuickDismiss, ctx),
    ]);
    cx.on_action(|_: &ToggleQuickCommand, cx| toggle(cx));
}

/// The editor state the palette sends to; set by the chat panel.
struct Target(WeakEntity<EditorState>);

impl Global for Target {}

pub fn register(state: &Entity<EditorState>, cx: &mut App) {
    cx.set_global(Target(state.downgrade()));
}

/// Where typing continues after a request went out from the bar.
struct ComposerTarget(WeakEntity<Composer>);

impl Global for ComposerTarget {}

pub fn register_composer(composer: &Entity<Composer>, cx: &mut App) {
    cx.set_global(ComposerTarget(composer.downgrade()));
}

fn toggle(cx: &mut App) {
    let Some(state) = cx.try_global::<Target>().and_then(|t| t.0.upgrade()) else {
        return;
    };
    let Some(window) = cx
        .active_window()
        .or_else(|| cx.windows().into_iter().next())
    else {
        return;
    };
    // Global action handlers run while the window is leased for dispatch.
    cx.defer(move |cx| open(window, state, cx));
}

fn open(window: AnyWindowHandle, state: Entity<EditorState>, cx: &mut App) {
    let _ = window.update(cx, |_, window, cx| {
        if window.has_active_dialog(cx) {
            window.close_dialog(cx);
            return;
        }
        let palette = cx.new(|cx| QuickCommand::new(state, window, cx));
        let first = Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if first.replace(false) {
                let query = palette.read(cx).query.clone();
                query.update(cx, |q, cx| q.focus(window, cx));
            }
            dialog
                .close_button(false)
                .margin_top(px(140.))
                .w(px(600.))
                .p_0()
                .gap_0()
                .child(palette.clone())
        });
    });
}

/// A row the palette can run.
enum Row {
    /// What was typed, sent as a request.
    Request(SharedString),
    /// A recipe: its slash name and title.
    Recipe(String, String),
}

struct QuickCommand {
    state: Entity<EditorState>,
    query: Entity<InputState>,
    /// The row Enter runs.
    selected: usize,
    _subscription: Subscription,
}

impl QuickCommand {
    fn new(state: Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query =
            cx.new(|cx| InputState::new(window, cx).placeholder("Ask the agent or pick a recipe"));
        let _subscription =
            cx.subscribe_in(&query, window, |this, _, event, window, cx| match event {
                InputEvent::Change => {
                    this.selected = 0;
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.run(this.selected, window, cx),
                _ => {}
            });
        Self {
            state,
            query,
            selected: 0,
            _subscription,
        }
    }

    fn rows(&self, cx: &App) -> Vec<Row> {
        let text = self.query.read(cx).value();
        let q = text.trim().trim_start_matches('/').to_lowercase();
        let request = (!text.trim().is_empty() && !text.starts_with('/'))
            .then(|| Row::Request(text.trim().to_owned().into()));
        request
            .into_iter()
            .chain(
                recipes::list(self.state.read(cx))
                    .into_iter()
                    .filter(|(name, title)| {
                        q.is_empty() || name.contains(&q) || title.to_lowercase().contains(&q)
                    })
                    .map(|(name, title)| Row::Recipe(name, title)),
            )
            .collect()
    }

    fn run(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let text = match self.rows(cx).into_iter().nth(ix) {
            Some(Row::Request(q)) => q.to_string(),
            Some(Row::Recipe(name, _)) => format!("/{name}"),
            None => return,
        };
        self.state.update(cx, |s, cx| s.send_chat(text, cx));
        window.close_dialog(cx);
        if let Some(composer) = cx
            .try_global::<ComposerTarget>()
            .and_then(|c| c.0.upgrade())
        {
            composer.update(cx, |c, cx| c.focus(window, cx));
        }
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let n = self.rows(cx).len() as isize;
        if n > 0 {
            self.selected = (self.selected as isize + delta).rem_euclid(n) as usize;
            cx.notify();
        }
    }
}

impl Render for QuickCommand {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows(cx);
        let selected = self.selected.min(rows.len().saturating_sub(1));
        let first_recipe = rows.iter().position(|r| matches!(r, Row::Recipe(..)));
        let items = rows.into_iter().enumerate().map(|(ix, row)| {
            let picked = ix == selected;
            let (icon, label, name) = match row {
                Row::Request(q) => (IconName::Sparkles, q, None),
                Row::Recipe(name, title) => (recipes::icon(&name), title.into(), Some(name)),
            };
            v_flex()
                .when(first_recipe == Some(ix), |el| {
                    el.child(
                        div()
                            .px(px(10.))
                            .py(px(6.))
                            .text_size(px(10.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t::T3)
                            .child("Recipes"),
                    )
                })
                .child(
                    h_flex()
                        .id(("quick-row", ix))
                        .h(px(30.))
                        .px(px(10.))
                        .gap(px(10.))
                        .rounded(t::RADIUS)
                        .border_1()
                        .cursor_pointer()
                        .map(|el| {
                            if picked {
                                el.bg(t::alpha(t::AC, 0.14))
                                    .border_color(t::alpha(t::AC, 0.3))
                            } else {
                                el.border_color(t::alpha(t::AC, 0.))
                                    .hover(|el| el.bg(t::HV))
                            }
                        })
                        .on_click(cx.listener(move |this, _, window, cx| this.run(ix, window, cx)))
                        .child(Icon::new(icon).color(if picked { t::AC } else { t::T2 }))
                        .child(div().flex_1().min_w_0().truncate().child(label))
                        .children(name.map(|name| {
                            div()
                                .flex_none()
                                .font_family(t::mono())
                                .text_size(px(11.))
                                .text_color(if picked { t::T2 } else { t::T3 })
                                .child(format!("/{name}"))
                        }))
                        .when(picked, |el| el.child(Kbd::new("Enter"))),
                )
        });
        let hint = |keys: &[&'static str], what: &'static str| {
            h_flex()
                .gap(px(4.))
                .children(keys.iter().map(|k| Kbd::new(*k)))
                .child(what)
        };
        v_flex()
            .key_context(CONTEXT)
            .on_action(cx.listener(|this, _: &QuickUp, _, cx| this.move_selection(-1, cx)))
            .on_action(cx.listener(|this, _: &QuickDown, _, cx| this.move_selection(1, cx)))
            .on_action(cx.listener(|_, _: &QuickDismiss, window, cx| window.close_dialog(cx)))
            .child(
                Input::new(&self.query)
                    .prefix(Icon::new(IconName::Sparkles).size(px(16.)).color(t::AC))
                    .suffix(div().text_color(t::T2).child(Kbd::new("Esc")))
                    .h(px(46.))
                    .px(px(14.))
                    .gap(px(10.))
                    .rounded(px(0.))
                    .border_0()
                    .bg(t::alpha(t::G, 0.))
                    .shadow(Vec::new())
                    .text_size(px(14.))
                    .line_height(px(20.)),
            )
            .child(
                v_flex()
                    .p(px(6.))
                    .border_t_1()
                    .border_color(t::LN)
                    .children(items),
            )
            .child(
                h_flex()
                    .h(px(32.))
                    .px(px(14.))
                    .gap(px(14.))
                    .border_t_1()
                    .border_color(t::LN)
                    .text_size(px(11.))
                    .text_color(t::T3)
                    .child(hint(&["Up", "Down"], "Move"))
                    .child(hint(&["Enter"], "Run"))
                    .child(div().flex_1())
                    .child(hint(&["Cmd K"], "Toggle")),
            )
    }
}
