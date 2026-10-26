//! Quick-command bar (PRD AG1): cmd/ctrl-K from anywhere opens a palette to
//! type a one-off request or start a recipe. Both go through `send_chat`.

use super::recipes::RECIPES;
use crate::state::EditorState;
use gpui_kit::component::command::{Command, CommandGroup, CommandItem, CommandState};
use gpui_kit::component::{IconName, IndexPath, WindowExt as _};
use gpui_kit::*;
use std::cell::Cell;

actions!(koo_chat, [ToggleQuickCommand]);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-k", ToggleQuickCommand, None),
        KeyBinding::new("ctrl-k", ToggleQuickCommand, None),
    ]);
    cx.on_action(|_: &ToggleQuickCommand, cx| toggle(cx));
}

/// The editor state the palette sends to; set by the chat panel.
struct Target(WeakEntity<EditorState>);

impl Global for Target {}

pub fn register(state: &Entity<EditorState>, cx: &mut App) {
    cx.set_global(Target(state.downgrade()));
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
                let command = palette.read(cx).command.clone();
                command.update(cx, |c, cx| c.focus(window, cx));
            }
            dialog
                .w(px(560.))
                .margin_top(px(140.))
                .close_button(false)
                .p_0()
                .child(palette.clone())
        });
    });
}

struct QuickCommand {
    state: Entity<EditorState>,
    command: Entity<CommandState>,
    query: SharedString,
}

impl QuickCommand {
    fn new(state: Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            state,
            command: cx.new(|cx| CommandState::new(window, cx)),
            query: SharedString::default(),
        }
    }

    fn recipes(&self) -> Vec<(&'static str, &'static str)> {
        let q = self.query.trim().trim_start_matches('/').to_lowercase();
        RECIPES
            .iter()
            .filter(|(name, title)| {
                q.is_empty() || name.contains(&q) || title.to_lowercase().contains(&q)
            })
            .copied()
            .collect()
    }

    fn has_request(&self) -> bool {
        !self.query.trim().is_empty() && !self.query.starts_with('/')
    }
}

impl Render for QuickCommand {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let recipes = self.recipes();
        let request = self.has_request().then(|| self.query.clone());
        let mut palette = Command::new(&self.command)
            .placeholder("Ask the agent or pick a recipe")
            .filterable(false)
            .bordered(false)
            .on_query({
                let this = cx.entity().downgrade();
                move |q: &str, _, cx| {
                    let _ = this.update(cx, |this, cx| {
                        this.query = q.to_string().into();
                        cx.notify();
                    });
                }
            })
            .on_confirm({
                let state = self.state.clone();
                let recipes = recipes.clone();
                let request = request.clone();
                move |ix: IndexPath, window, cx| {
                    let text = match (&request, ix.section) {
                        (Some(q), 0) => q.to_string(),
                        _ => match recipes.get(ix.row) {
                            Some((name, _)) => format!("/{name}"),
                            None => return,
                        },
                    };
                    state.update(cx, |s, cx| s.send_chat(text, cx));
                    window.close_dialog(cx);
                }
            })
            .on_cancel(|window, cx| window.close_dialog(cx));
        if let Some(q) = request {
            palette = palette
                .group(CommandGroup::new().item(CommandItem::new().icon(IconName::Bot).label(q)));
        }
        if !recipes.is_empty() {
            palette = palette.group(
                CommandGroup::new().label("Recipes").items(
                    recipes
                        .iter()
                        .map(|(name, title)| CommandItem::new().label(format!("{title}  /{name}"))),
                ),
            );
        }
        palette
    }
}
