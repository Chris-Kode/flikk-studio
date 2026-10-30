//! Message input: Enter sends, Shift+Enter breaks the line, `@` opens the
//! reference list (inserted as chips) and `/` at the start lists recipes.
//! Clips, media, FX and transcript words dropped on it, and references
//! other panels queue with "Add to chat", become chips too.

use super::recipes;
use super::refs::{self, RefKind, Reference};
use crate::state::EditorState;
use crate::ui::{AudioFxDrag, FxDrag, MediaDrag, RefDrag};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InlineToken, InputEvent, InputToken, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::agent::{AgentModel, Backend, ModelChoice, effort_label};
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
    /// Playhead, In/Out or the selection: one or more references at once.
    Special {
        kind: RefKind,
        label: String,
        detail: String,
        refs: Vec<(String, SharedString)>,
    },
    Recipe {
        name: String,
        title: String,
    },
}

impl Suggestion {
    /// The group it's listed under, if any.
    fn group(&self) -> Option<RefKind> {
        match self {
            Self::Reference(r) => Some(r.kind),
            Self::Special { .. } | Self::Recipe { .. } => None,
        }
    }
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
    list_scroll: ScrollHandle,
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
            cx.observe_in(&state, window, |this, state, window, cx| {
                let refs = state.update(cx, |s, _| s.take_mentions());
                this.insert_refs(refs, window, cx);
            }),
        ];
        let mut this = Self {
            state,
            input,
            suggestions: None,
            list_scroll: ScrollHandle::new(),
            _subscriptions,
        };
        // References queued before the composer existed.
        let refs = this.state.update(cx, |s, _| s.take_mentions());
        this.insert_refs(refs, window, cx);
        this.demo(window, cx);
        this
    }

    /// Insert references as chips at the cursor and focus the input.
    pub fn insert_refs(
        &mut self,
        refs: Vec<(String, SharedString)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if refs.is_empty() {
            return;
        }
        self.suggestions = None;
        self.input.update(cx, |input, cx| {
            for (text, label) in refs {
                let value = input.value();
                let cursor = input.cursor().min(value.len());
                if value[..cursor]
                    .chars()
                    .next_back()
                    .is_some_and(|c| !c.is_whitespace())
                {
                    input.insert(" ", window, cx);
                }
                let at = input.cursor();
                let token = InlineToken::new(text.clone(), text.clone()).with_label(label);
                if input
                    .replace_range_with_token(at..at, token, window, cx)
                    .is_err()
                {
                    input.insert(text, window, cx);
                }
                input.insert(" ", window, cx);
            }
            input.focus(window, cx);
        });
        cx.notify();
    }

    /// Demo only (`KOOEDITOR_DEMO_CHAT_PICKER=@query`): open the `@` list.
    fn demo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !crate::demo::enabled() {
            return;
        }
        if let Ok(query) = std::env::var("KOOEDITOR_DEMO_CHAT_PICKER") {
            self.input.update(cx, |input, cx| {
                let text = format!("Trim {query}");
                let end = text.len();
                input.set_value(text, window, cx);
                input.set_selected_range(end..end, cx);
            });
            self.refresh_suggestions(cx);
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
            let state = self.state.read(cx);
            let mut items = specials(state, query);
            items.extend(
                refs::search(state.bus.project(), query)
                    .into_iter()
                    .map(Suggestion::Reference),
            );
            items
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
                    let token = InlineToken::new(r.token_text(), r.token_text())
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
                Suggestion::Special { refs, .. } => {
                    input.set_selected_range(s.range.clone(), cx);
                    input.replace("", window, cx);
                    for (text, label) in refs {
                        let at = input.cursor();
                        let token =
                            InlineToken::new(text.clone(), text.clone()).with_label(label.clone());
                        if input
                            .replace_range_with_token(at..at, token, window, cx)
                            .is_err()
                        {
                            input.insert(text.clone(), window, cx);
                        }
                        input.insert(" ", window, cx);
                    }
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
        // Rows include a header before each group.
        let headers = (0..=s.selected)
            .filter(|&i| starts_group(&s.items, i))
            .count();
        self.list_scroll.scroll_to_item(s.selected + headers);
        cx.notify();
        true
    }

    /// Model and thinking level for the current backend, as its CLI lists
    /// them, applied from the next message on.
    fn render_model_pickers(&self, cx: &App) -> impl IntoElement {
        let s = self.state.read(cx);
        let backend = s.settings.backend;
        let choice = s.settings.models.get(backend).clone();
        let models = s.agent_models.get(&backend).cloned().unwrap_or_default();
        let efforts = efforts_of(&models, choice.model.as_deref()).to_vec();
        let model_text = match &choice.model {
            Some(id) => models
                .iter()
                .find(|m| &m.id == id)
                .map_or_else(|| id.clone(), |m| m.label.clone()),
            None => "Default".to_owned(),
        };
        let effort_text = choice
            .effort
            .as_deref()
            .map_or_else(|| "Auto".to_owned(), effort_label);
        let state = self.state.clone();
        let model_picker = Button::new("chat-model")
            .label(model_text)
            .ghost()
            .xsmall()
            .dropdown_caret(true)
            .tooltip("Model")
            .dropdown_menu_with_anchor(Anchor::BottomLeft, {
                let (state, choice) = (state.clone(), choice.clone());
                move |menu, _, _| {
                    let item = |label: String, id: Option<String>| {
                        // A level the new model doesn't take goes back to Auto.
                        let keep = choice
                            .effort
                            .as_ref()
                            .is_some_and(|e| efforts_of(&models, id.as_deref()).contains(e));
                        let state = state.clone();
                        PopupMenuItem::new(label)
                            .checked(id == choice.model)
                            .on_click(move |_, _, cx| {
                                let id = id.clone();
                                set_choice(&state, backend, cx, move |c| {
                                    c.model = id;
                                    if !keep {
                                        c.effort = None;
                                    }
                                })
                            })
                    };
                    let mut menu = menu
                        .max_h(px(360.))
                        .scrollable(true)
                        .item(item("Default".into(), None));
                    if !models.is_empty() {
                        menu = menu.separator();
                    }
                    for m in &models {
                        menu = menu.item(item(m.label.clone(), Some(m.id.clone())));
                    }
                    menu
                }
            })
            .on_open_change({
                let state = state.clone();
                move |open, _, cx| {
                    if *open {
                        state.update(cx, |s, cx| s.load_agent_models(cx));
                    }
                }
            });
        let effort_picker = (!efforts.is_empty()).then(|| {
            Button::new("chat-effort")
                .label(effort_text)
                .ghost()
                .xsmall()
                .dropdown_caret(true)
                .tooltip("Thinking")
                .dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, _, _| {
                    let item = |label: String, effort: Option<String>| {
                        let state = state.clone();
                        PopupMenuItem::new(label)
                            .checked(effort == choice.effort)
                            .on_click(move |_, _, cx| {
                                let effort = effort.clone();
                                set_choice(&state, backend, cx, move |c| c.effort = effort)
                            })
                    };
                    let mut menu = menu.item(item("Auto".into(), None)).separator();
                    for effort in &efforts {
                        menu = menu.item(item(effort_label(effort), Some(effort.clone())));
                    }
                    menu
                })
        });
        h_flex()
            .min_w_0()
            .gap_0p5()
            .child(model_picker)
            .children(effort_picker)
    }

    fn render_suggestions(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let s = self.suggestions.as_ref()?;
        let theme = cx.theme();
        let mut rows: Vec<AnyElement> = Vec::new();
        for (ix, item) in s.items.iter().enumerate() {
            if starts_group(&s.items, ix)
                && let Some(kind) = item.group()
            {
                rows.push(
                    div()
                        .flex_none()
                        .px_2()
                        .pt_1p5()
                        .pb_0p5()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(kind.title())
                        .into_any_element(),
                );
            }
            let (icon, label, detail) = match item {
                Suggestion::Reference(r) => {
                    (Some(r.kind.icon()), r.label.clone(), r.detail.clone())
                }
                Suggestion::Special {
                    kind,
                    label,
                    detail,
                    ..
                } => (Some(kind.icon()), label.clone(), detail.clone()),
                Suggestion::Recipe { name, title } => (None, format!("/{name}"), title.clone()),
            };
            let row = h_flex()
                .id(("suggestion", ix))
                .flex_none()
                .h(px(28.))
                .px_2()
                .gap_2()
                .rounded(theme.radius)
                .text_sm()
                .when(ix == s.selected, |el| el.bg(theme.accent))
                .hover(|el| el.bg(theme.accent))
                .cursor_pointer()
                .when_some(icon, |el, icon| {
                    el.child(
                        Icon::new(icon)
                            .size_3p5()
                            .flex_none()
                            .text_color(theme.muted_foreground),
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
                );
            rows.push(row.into_any_element());
        }
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
                    .max_h(px(300.))
                    .overflow_y_scroll()
                    .track_scroll(&self.list_scroll)
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
        let (ring, drop) = (theme.ring, theme.accent.opacity(0.4));
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
                    .drag_over::<RefDrag>(move |s, _, _, _| s.border_color(ring).bg(drop))
                    .drag_over::<MediaDrag>(move |s, _, _, _| s.border_color(ring).bg(drop))
                    .drag_over::<FxDrag>(move |s, _, _, _| s.border_color(ring).bg(drop))
                    .drag_over::<AudioFxDrag>(move |s, _, _, _| s.border_color(ring).bg(drop))
                    .on_drop(cx.listener(|this, drag: &RefDrag, window, cx| {
                        this.insert_refs(drag.refs.clone(), window, cx)
                    }))
                    .on_drop(cx.listener(|this, drag: &MediaDrag, window, cx| {
                        let token = format!("@media {}", drag.media);
                        this.insert_refs(vec![(token, drag.name.clone())], window, cx)
                    }))
                    .on_drop(cx.listener(|this, drag: &FxDrag, window, cx| {
                        let Some(fx) = drag.program(&this.state, cx) else {
                            return;
                        };
                        let token = format!("@fx {fx}");
                        this.insert_refs(vec![(token, drag.name.clone())], window, cx)
                    }))
                    .on_drop(cx.listener(|this, drag: &AudioFxDrag, window, cx| {
                        let token = format!("@audiofx {}", drag.kind.name());
                        this.insert_refs(vec![(token, drag.name.clone())], window, cx)
                    }))
                    .child(
                        Textarea::new(&self.input)
                            .appearance(false)
                            .text_sm()
                            .px_1()
                            .pt_1()
                            .token(|token, _, _| {
                                let chip = InputToken::new(token);
                                match RefKind::of_token(token.token().text()) {
                                    Some(kind) => chip.icon(kind.icon()),
                                    None => chip,
                                }
                            }),
                    )
                    .child(
                        h_flex()
                            .px_1p5()
                            .pb_1p5()
                            .gap_2()
                            .justify_between()
                            .child(self.render_model_pickers(cx))
                            .child(action),
                    ),
            )
            .children(self.render_suggestions(cx))
    }
}

/// Thinking levels of model `id`, or of the CLI's default model for `None`.
fn efforts_of<'a>(models: &'a [AgentModel], id: Option<&str>) -> &'a [String] {
    models
        .iter()
        .find(|m| match id {
            Some(id) => m.id == id,
            None => m.default,
        })
        .map_or(&[], |m| &m.efforts)
}

fn set_choice(
    state: &Entity<EditorState>,
    backend: Backend,
    cx: &mut App,
    f: impl FnOnce(&mut ModelChoice),
) {
    state.update(cx, |s, cx| {
        let mut settings = s.settings.clone();
        f(settings.models.get_mut(backend));
        if settings != s.settings {
            s.set_settings(settings, cx);
        }
    });
}

/// Whether item `ix` opens a new group (and gets a header row).
fn starts_group(items: &[Suggestion], ix: usize) -> bool {
    let group = items[ix].group();
    group.is_some() && (ix == 0 || items[ix - 1].group() != group)
}

/// Playhead, In/Out (when set) and the selection (when any), matching the
/// typed query like the other references.
fn specials(state: &EditorState, query: &str) -> Vec<Suggestion> {
    let words = refs::query_words(query);
    let project = state.bus.project();
    let mut out = Vec::new();
    let (token, label) = refs::time_token(state.playhead);
    out.push(Suggestion::Special {
        kind: RefKind::Time,
        label: "Playhead".into(),
        detail: label.clone(),
        refs: vec![(token, label.into())],
    });
    if let Some(range) = state.in_out.filter(|r| !r.is_empty()) {
        let (token, label) = refs::range_token(range);
        out.push(Suggestion::Special {
            kind: RefKind::Range,
            label: "In/Out".into(),
            detail: label.clone(),
            refs: vec![(token, label.into())],
        });
    }
    let mut selected: Vec<(String, SharedString)> = state
        .selection
        .clips
        .iter()
        .filter_map(|id| project.timeline.clip(*id))
        .map(|(_, c)| (format!("@clip {}", c.id), c.display_name(project).into()))
        .collect();
    if let Some((clip, edge)) = state.selected_transition {
        let side = match edge {
            koo_core::command::Edge::Start => "in",
            koo_core::command::Edge::End => "out",
        };
        let token = format!("@transition {clip}/{side}");
        let resolved = koo_core::query::resolve_project_references(project, &token);
        if let Some(r) = resolved.first() {
            let m = refs::describe(project, &token, &r.reference);
            if m.resolved {
                selected.push((token, m.label.into()));
            }
        }
    }
    if !selected.is_empty() {
        let detail = match selected.as_slice() {
            [(_, one)] => one.to_string(),
            many => format!("{} items", many.len()),
        };
        out.push(Suggestion::Special {
            kind: RefKind::Clip,
            label: "Selection".into(),
            detail,
            refs: selected,
        });
    }
    out.retain(|s| match s {
        Suggestion::Special {
            kind,
            label,
            detail,
            ..
        } => refs::matches(&words, &format!("{label} {} {detail}", kind.word())),
        _ => true,
    });
    out
}
