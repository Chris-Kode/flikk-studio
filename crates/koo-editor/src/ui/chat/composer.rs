//! Message input: Enter sends, Shift+Enter breaks the line, `@` opens the
//! reference list (inserted as chips) and `/` at the start lists recipes.

use super::recipes;
use super::refs::{self, Reference};
use crate::state::EditorState;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InlineToken, InputEvent, Textarea, TextareaState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::ops::Range;

actions!(
    koo_chat,
    [SuggestUp, SuggestDown, SuggestAccept, SuggestDismiss]
);

const CONTEXT: &str = "ChatComposer";

pub fn init(cx: &mut App) {
    // Same depth as the input's own bindings and registered later, so these
    // win while the list is open; otherwise they propagate to the input.
    let ctx = Some("ChatComposer > Input");
    cx.bind_keys([
        KeyBinding::new("up", SuggestUp, ctx),
        KeyBinding::new("down", SuggestDown, ctx),
        KeyBinding::new("enter", SuggestAccept, ctx),
        KeyBinding::new("tab", SuggestAccept, ctx),
        KeyBinding::new("escape", SuggestDismiss, ctx),
    ]);
}

pub enum ComposerEvent {
    Submit(String),
}

impl EventEmitter<ComposerEvent> for Composer {}

enum Suggestion {
    Reference(Reference),
    Recipe { name: String, title: String },
}

struct Suggestions {
    /// Bytes of the `@query` or `/query` being completed.
    range: Range<usize>,
    items: Vec<Suggestion>,
    selected: usize,
}

pub struct Composer {
    state: Entity<EditorState>,
    input: Entity<TextareaState>,
    suggestions: Option<Suggestions>,
    _subscriptions: Vec<Subscription>,
}

impl Composer {
    pub fn new(state: Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 8)
                .submit_on_enter(true)
                .placeholder("Ask for an edit, @ to reference, / for recipes")
        });
        let _subscriptions = vec![
            cx.subscribe_in(&input, window, |this, _, event, window, cx| match event {
                InputEvent::Change => this.refresh_suggestions(cx),
                InputEvent::PressEnter { shift: false, .. } => this.submit(window, cx),
                InputEvent::Blur => {
                    this.suggestions = None;
                    cx.notify();
                }
                _ => {}
            }),
            cx.observe(&state, |_, _, cx| cx.notify()),
        ];
        Self {
            state,
            input,
            suggestions: None,
            _subscriptions,
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().trim().to_string();
        if text.is_empty() || self.state.read(cx).agent_busy() {
            return;
        }
        self.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.suggestions = None;
        cx.emit(ComposerEvent::Submit(text));
    }

    fn refresh_suggestions(&mut self, cx: &mut Context<Self>) {
        self.suggestions = self.find_suggestions(cx);
        cx.notify();
    }

    fn find_suggestions(&self, cx: &App) -> Option<Suggestions> {
        let input = self.input.read(cx);
        let text = input.value();
        let cursor = input.cursor().min(text.len());
        let before = text.get(..cursor)?;
        let start = before
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_whitespace())
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        if input
            .tokens()
            .iter()
            .any(|t| t.range().start < cursor && t.range().end > start)
        {
            return None;
        }
        let word = &before[start..];
        let items: Vec<Suggestion> = if let Some(query) = word.strip_prefix('@') {
            let project = self.state.read(cx).bus.project();
            refs::search(project, query)
                .into_iter()
                .map(Suggestion::Reference)
                .collect()
        } else {
            let query = word.strip_prefix('/').filter(|_| start == 0)?;
            recipes::list(self.state.read(cx))
                .into_iter()
                .filter(|(name, _)| name.starts_with(query))
                .map(|(name, title)| Suggestion::Recipe { name, title })
                .collect()
        };
        (!items.is_empty()).then_some(Suggestions {
            range: start..cursor,
            items,
            selected: 0,
        })
    }

    fn accept(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.suggestions.take() else {
            return;
        };
        let Some(item) = s.items.get(ix) else {
            return;
        };
        self.input.update(cx, |input, cx| {
            match item {
                Suggestion::Reference(r) => {
                    let token =
                        InlineToken::new(format!("{}:{}", r.kind.word(), r.id), r.token_text())
                            .with_label(r.label.clone());
                    if input
                        .replace_range_with_token(s.range.clone(), token, window, cx)
                        .is_err()
                    {
                        input.set_selected_range(s.range.clone(), cx);
                        input.replace(r.token_text(), window, cx);
                    }
                    input.insert(" ", window, cx);
                }
                Suggestion::Recipe { name, .. } => {
                    input.set_selected_range(s.range.clone(), cx);
                    input.replace(format!("/{name} "), window, cx);
                }
            }
            input.focus(window, cx);
        });
        self.suggestions = None;
        cx.notify();
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) -> bool {
        let Some(s) = self.suggestions.as_mut() else {
            return false;
        };
        let n = s.items.len() as isize;
        s.selected = (s.selected as isize + delta).rem_euclid(n) as usize;
        cx.notify();
        true
    }

    fn render_suggestions(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let s = self.suggestions.as_ref()?;
        let theme = cx.theme();
        let rows = s.items.iter().enumerate().map(|(ix, item)| {
            let (kind, label, detail) = match item {
                Suggestion::Reference(r) => {
                    (r.kind.word().to_string(), r.label.clone(), r.detail.clone())
                }
                Suggestion::Recipe { name, title } => {
                    (String::new(), format!("/{name}"), title.clone())
                }
            };
            h_flex()
                .id(("suggestion", ix))
                .h(px(28.))
                .px_2()
                .gap_2()
                .rounded(theme.radius)
                .text_sm()
                .when(ix == s.selected, |el| el.bg(theme.accent))
                .hover(|el| el.bg(theme.accent))
                .cursor_pointer()
                .when(!kind.is_empty(), |el| {
                    el.child(
                        div()
                            .w(px(44.))
                            .flex_none()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(kind),
                    )
                })
                .child(div().flex_1().min_w_0().truncate().child(label))
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(detail),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.accept(ix, window, cx)
                    }),
                )
        });
        Some(
            deferred(
                v_flex()
                    .id("suggestions")
                    .absolute()
                    .bottom_full()
                    .left_0()
                    .right_0()
                    .mb_1()
                    .p_1()
                    .max_h(px(260.))
                    .overflow_y_scroll()
                    .rounded(theme.radius_lg)
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.popover)
                    .text_color(theme.popover_foreground)
                    .shadow_lg()
                    .children(rows),
            )
            .with_priority(1),
        )
    }
}

impl Render for Composer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.state.read(cx).agent_busy();
        let empty = self.input.read(cx).value().trim().is_empty();
        let theme = cx.theme();
        let action = if busy {
            Button::new("stop")
                .icon(IconName::Square)
                .small()
                .tooltip("Stop")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.state.update(cx, |s, cx| s.cancel_agent(cx));
                }))
        } else {
            Button::new("send")
                .icon(IconName::ArrowUp)
                .small()
                .primary()
                .disabled(empty)
                .tooltip("Send")
                .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)))
        };
        div()
            .relative()
            .p_2()
            .key_context(CONTEXT)
            .on_action(cx.listener(|this, _: &SuggestUp, _, cx| {
                if !this.move_selection(-1, cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &SuggestDown, _, cx| {
                if !this.move_selection(1, cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &SuggestAccept, window, cx| {
                match this.suggestions.as_ref().map(|s| s.selected) {
                    Some(ix) => this.accept(ix, window, cx),
                    None => cx.propagate(),
                }
            }))
            .on_action(cx.listener(|this, _: &SuggestDismiss, _, cx| {
                if this.suggestions.take().is_some() {
                    cx.notify();
                } else {
                    cx.propagate();
                }
            }))
            .child(
                v_flex()
                    .rounded(theme.radius_lg)
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.background)
                    .child(
                        Textarea::new(&self.input)
                            .appearance(false)
                            .text_sm()
                            .px_1()
                            .pt_1(),
                    )
                    .child(h_flex().px_1p5().pb_1p5().justify_end().child(action)),
            )
            .children(self.render_suggestions(cx))
    }
}
