//! Message input: Enter sends, Shift+Enter breaks the line, `@` opens the
//! reference list (inserted as chips) and `/` at the start lists skills.
//! The style picker beside send sets the style every message is sent with.
//! Clips, media, FX and transcript words dropped on it, and references
//! other panels queue with "Add to chat", become chips too. Files and
//! folders picked with the paperclip or dropped from the system are
//! attached to the chat, for the agent to read, and shown above the input.

use super::chips::{chip, mark_element};
use super::refs::{self, Mark, RefKind, Reference};
use crate::state::EditorState;
use crate::ui::editor_settings::{EditorSettings, Focus};
use crate::ui::kit::popover::surface;
use crate::ui::kit::{
    Button, DropdownMenu as _, Icon, IconName, Kbd, PopupMenuItem, ScrollableElement as _,
    Textarea, Tooltip, h_flex, theme as t, v_flex,
};
use crate::ui::settings::efforts_of;
use crate::ui::{AudioFxDrag, FxDrag, MediaDrag, RefDrag, SoundDrag};
use flikk_core::agent::{Attachment, AttachmentKind, Backend, ModelChoice, effort_label};
use flikk_core::skills::{self, Kind};
use gpui_kit::base::input::{InlineToken, InputEvent, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::ops::Range;

actions!(
    flikk_chat,
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
    Skill {
        name: String,
        description: String,
    },
    /// Opens Settings on a new skill.
    NewSkill,
}

impl Suggestion {
    /// The group it's listed under, if any.
    fn group(&self) -> Option<RefKind> {
        match self {
            Self::Reference(r) => Some(r.kind),
            Self::Special { .. } | Self::Skill { .. } | Self::NewSkill => None,
        }
    }

    fn mark(&self, project: &flikk_core::model::Project) -> Option<Mark> {
        match self {
            Self::Reference(r) => Some(r.kind.mark(project, &r.token_text())),
            Self::Special { kind, refs, .. } => {
                let token = refs.first().map_or("", |(text, _)| text.as_str());
                Some(kind.mark(project, token))
            }
            Self::Skill { .. } | Self::NewSkill => None,
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
                .placeholder("Ask for an edit, @ to reference, / for skills")
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

    /// Demo only (`FLIKK_DEMO_CHAT_PICKER=@query`): open the `@` list.
    fn demo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !crate::demo::enabled() {
            return;
        }
        if let Ok(query) = std::env::var("FLIKK_DEMO_CHAT_PICKER") {
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

    /// Whether typing goes into the message.
    pub fn has_focus(&self, window: &Window, cx: &App) -> bool {
        self.input.read(cx).has_selection_focus(window, cx)
    }

    /// Type `@` at the cursor, which opens the reference list.
    fn start_reference(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| {
            let value = input.value();
            let cursor = input.cursor().min(value.len());
            if value[..cursor]
                .chars()
                .next_back()
                .is_some_and(|c| !c.is_whitespace())
            {
                input.insert(" ", window, cx);
            }
            input.insert("@", window, cx);
            input.focus(window, cx);
        });
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
            skills::list(Kind::Skill)
                .into_iter()
                .filter(|e| e.name.starts_with(query))
                .map(|e| Suggestion::Skill {
                    name: e.name,
                    description: e.description,
                })
                .chain([Suggestion::NewSkill])
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
        if matches!(item, Suggestion::NewSkill) {
            self.input.update(cx, |input, cx| {
                input.set_selected_range(s.range.clone(), cx);
                input.replace("", window, cx);
            });
            cx.notify();
            EditorSettings::open_entries(Kind::Skill, Focus::New, window, cx);
            return;
        }
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
                Suggestion::Skill { name, .. } => {
                    input.set_selected_range(s.range.clone(), cx);
                    input.replace(format!("/{name} "), window, cx);
                }
                Suggestion::NewSkill => {}
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
            .small()
            .trailing_icon(IconName::ChevronDown)
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
                        .item(PopupMenuItem::label("Model"))
                        .item(item("Default".into(), None));
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
                .icon(IconName::Brain)
                .label(effort_text)
                .ghost()
                .small()
                .trailing_icon(IconName::ChevronDown)
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
                    let mut menu = menu
                        .item(PopupMenuItem::label("Thinking"))
                        .item(item("Auto".into(), None));
                    for effort in &efforts {
                        menu = menu.item(item(effort_label(effort), Some(effort.clone())));
                    }
                    menu
                })
        });
        h_flex()
            .min_w_0()
            .gap(px(2.))
            .child(model_picker)
            .children(effort_picker)
    }

    /// The style messages are sent with: its name, or just the icon when
    /// there is none.
    fn render_style_picker(&self, cx: &App) -> impl IntoElement {
        let styles = skills::list(Kind::Style);
        let picked = self.state.read(cx).settings.style.clone();
        let current = picked
            .as_deref()
            .and_then(|id| styles.iter().find(|e| e.id == id));
        let state = self.state.clone();
        let button = Button::new("chat-style")
            .icon(IconName::Palette)
            .ghost()
            .small()
            .tooltip("Style");
        let button = match current {
            Some(style) => button
                .label(style.name.clone())
                .trailing_icon(IconName::ChevronDown)
                .selected(true),
            None => button,
        };
        let picked = current.map(|e| e.id.clone());
        div()
            .flex_none()
            .max_w(px(140.))
            .child(
                button.dropdown_menu_with_anchor(Anchor::BottomRight, move |menu, _, _| {
                    let item = |label: String, id: Option<String>| {
                        let state = state.clone();
                        PopupMenuItem::new(label)
                            .checked(id == picked)
                            .on_click(move |_, _, cx| {
                                let id = id.clone();
                                state.update(cx, |s, cx| {
                                    let mut settings = s.settings.clone();
                                    settings.style = id;
                                    if settings != s.settings {
                                        s.set_settings(settings, cx);
                                    }
                                })
                            })
                    };
                    let mut menu = menu
                        .max_h(px(360.))
                        .item(PopupMenuItem::label("Style"))
                        .item(item("No style".into(), None));
                    for style in &styles {
                        menu = menu.item(item(style.name.clone(), Some(style.id.clone())));
                    }
                    menu.item(PopupMenuItem::separator()).item(
                        PopupMenuItem::new("Manage styles").on_click(|_, window, cx| {
                            EditorSettings::open_entries(Kind::Style, Focus::Last, window, cx)
                        }),
                    )
                }),
            )
    }

    fn render_suggestions(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let s = self.suggestions.as_ref()?;
        let project = self.state.read(cx).bus.project();
        let mut rows: Vec<AnyElement> = Vec::new();
        for (ix, item) in s.items.iter().enumerate() {
            if starts_group(&s.items, ix)
                && let Some(kind) = item.group()
            {
                rows.push(
                    v_flex()
                        .flex_none()
                        .when(ix > 0, |el| {
                            el.child(div().h(px(1.)).mx(px(-4.)).my(px(4.)).bg(t::LN))
                        })
                        .child(group_title(kind.title()))
                        .into_any_element(),
                );
            }
            if ix == 0 && matches!(item, Suggestion::Skill { .. }) {
                rows.push(group_title("Skills").into_any_element());
            }
            let selected = ix == s.selected;
            let row = h_flex()
                .id(("suggestion", ix))
                .flex_none()
                .h(px(26.))
                .px(px(8.))
                .gap(px(8.))
                .rounded(t::RADIUS)
                .cursor_pointer()
                .when(selected, |el| el.bg(t::HV))
                .hover(|el| el.bg(t::HV))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.accept(ix, window, cx)
                    }),
                );
            let row = match item {
                Suggestion::Skill { name, description } => row
                    .child(
                        div()
                            .flex_none()
                            .w(px(104.))
                            .truncate()
                            .font_family(t::mono())
                            .text_size(px(11.))
                            .child(format!("/{name}")),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(t::T2)
                            .child(description.clone()),
                    )
                    .when(selected, |el| el.child(Kbd::new("Tab"))),
                Suggestion::NewSkill => {
                    if ix > 0 {
                        rows.push(
                            div()
                                .flex_none()
                                .h(px(1.))
                                .mx(px(-4.))
                                .my(px(4.))
                                .bg(t::LN)
                                .into_any_element(),
                        );
                    }
                    row.text_color(t::T2)
                        .child(Icon::new(IconName::Plus).size(px(13.)))
                        .child(div().flex_1().child("New skill"))
                        .when(selected, |el| el.child(Kbd::new("Enter")))
                }
                Suggestion::Reference(Reference { label, detail, .. })
                | Suggestion::Special { label, detail, .. } => row
                    .children(item.mark(project).map(|m| mark_element(m, px(8.), px(12.))))
                    .child(div().flex_1().min_w_0().truncate().child(label.clone()))
                    .child(
                        div()
                            .flex_none()
                            .font_family(t::mono())
                            .text_size(px(10.))
                            .text_color(t::T3)
                            .child(detail.clone()),
                    )
                    .when(selected, |el| el.child(Kbd::new("Enter"))),
            };
            rows.push(row.into_any_element());
        }
        Some(
            deferred(
                surface()
                    .absolute()
                    .bottom_full()
                    .left_0()
                    .right_0()
                    .mb(px(6.))
                    .overflow_hidden()
                    .child(
                        v_flex()
                            .id("suggestions")
                            .max_h(px(300.))
                            .p(px(4.))
                            .track_scroll(&self.list_scroll)
                            .overflow_y_scroll()
                            .children(rows),
                    )
                    .vertical_scrollbar(&self.list_scroll),
            )
            .with_priority(1),
        )
    }

    /// Pick files and folders to attach to the open chat. The pick goes to
    /// the chat it was made in, or nowhere if that is no longer open.
    fn prompt_attach(&mut self, cx: &mut Context<Self>) {
        let (project, chat) = self.open_chat(cx);
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                this.update(cx, |this, cx| {
                    this.state
                        .update(cx, |s, cx| s.attach(project, &chat, paths, cx))
                })
                .ok();
            }
        })
        .detach();
    }

    fn open_chat(&self, cx: &App) -> (flikk_core::ProjectId, String) {
        let s = self.state.read(cx);
        (s.bus.project().id, s.active_chat().to_owned())
    }

    /// The chat's attachments: a chip each, with the full path on hover
    /// and a cross to remove it. Crossed out when the path is gone.
    fn render_attachments(&self, cx: &mut Context<Self>) -> Option<Div> {
        let state = self.state.read(cx);
        // Fixed while the agent replies: the turn reads what it was sent with.
        let locked = state.agent_busy();
        let attachments = state.attachments().to_vec();
        if attachments.is_empty() {
            return None;
        }
        let chips = attachments.into_iter().enumerate().map(|(ix, a)| {
            let Attachment { id, kind, .. } = a;
            let exists = a.path.exists();
            let icon = match kind {
                AttachmentKind::Folder => IconName::Folder,
                AttachmentKind::File => IconName::File,
            };
            let group: SharedString = format!("chat-attachment-{ix}").into();
            let state = self.state.clone();
            chip(Some(Mark::Icon(icon, t::T2)), a.name())
                .id(("chat-attachment", ix))
                .group(group.clone())
                .pr(px(3.))
                .when(!exists, |el| el.text_color(t::T3).line_through())
                .tooltip(Tooltip::text(a.path.display().to_string()))
                .when(!locked, |el| {
                    el.child(
                        div()
                            .id(("chat-attachment-remove", ix))
                            .flex()
                            .flex_none()
                            .items_center()
                            .invisible()
                            .group_hover(group, |s| s.visible())
                            .cursor_pointer()
                            .text_color(t::T3)
                            .hover(|s| s.text_color(t::T1))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .tooltip(Tooltip::text("Remove"))
                            .child(Icon::new(IconName::X).size(px(11.)))
                            .on_click(move |_, _, cx| state.update(cx, |s, cx| s.detach(id, cx))),
                    )
                })
        });
        Some(h_flex().flex_wrap().gap(px(4.)).px(px(2.)).children(chips))
    }

    /// While something is dragged over the composer: the dashed target it
    /// drops into, inserting chips, or attaching files from the system.
    fn render_drop_target(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let show = |s: StyleRefinement| s.opacity(1.);
        let can_attach = !self.state.read(cx).agent_busy();
        v_flex()
            .absolute()
            .inset_0()
            .justify_center()
            .items_center()
            .gap(px(6.))
            .rounded(t::RADIUS)
            .border_1()
            .border_dashed()
            .border_color(t::AC)
            .bg(t::alpha(t::AC, 0.08))
            .text_color(t::AC)
            .font_weight(FontWeight::MEDIUM)
            .opacity(0.)
            .drag_over::<RefDrag>(move |s, _, _, _| show(s))
            .drag_over::<MediaDrag>(move |s, _, _, _| show(s))
            .drag_over::<FxDrag>(move |s, _, _, _| show(s))
            .drag_over::<AudioFxDrag>(move |s, _, _, _| show(s))
            .drag_over::<SoundDrag>(move |s, _, _, _| show(s))
            // Not while the agent replies: see `EditorState::attach`.
            .when(can_attach, |el| {
                el.drag_over::<ExternalPaths>(move |s, _, _, _| show(s))
                    .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                        let (project, chat) = this.open_chat(cx);
                        let paths = paths.paths().to_vec();
                        this.state
                            .update(cx, |s, cx| s.attach(project, &chat, paths, cx))
                    }))
            })
            .on_drop(cx.listener(|this, drag: &RefDrag, window, cx| {
                this.insert_refs(drag.refs.clone(), window, cx)
            }))
            .on_drop(cx.listener(|this, drag: &MediaDrag, window, cx| {
                let project = this.state.read(cx).bus.project();
                let refs = drag
                    .all()
                    .into_iter()
                    .map(|id| {
                        let name = project.media(id).map_or(drag.name.clone(), |m| {
                            crate::ui::format::item_name(m).into()
                        });
                        (format!("@media {id}"), name)
                    })
                    .collect();
                this.insert_refs(refs, window, cx)
            }))
            .on_drop(cx.listener(|this, drag: &SoundDrag, window, cx| {
                let media = this.state.update(cx, |s, cx| {
                    crate::ui::library::shared::use_sound(s, drag.sound, cx)
                });
                let Some(media) = media else {
                    return;
                };
                let token = format!("@media {media}");
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
            .child(Icon::new(IconName::MessageSquarePlus).size(px(18.)))
            .child("Add to chat")
    }
}

impl Render for Composer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.state.read(cx).agent_busy();
        let empty = self.input.read(cx).value().trim().is_empty();
        let focused = self.has_focus(window, cx);
        let action = if busy {
            div()
                .id("stop")
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(24.))
                .rounded(t::RADIUS)
                .bg(t::T1)
                .cursor_pointer()
                .hover(|s| s.bg(t::T2))
                .tooltip(Tooltip::text("Stop"))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.state.update(cx, |s, cx| s.cancel_agent(cx));
                }))
                .child(div().size(px(8.)).rounded(px(1.5)).bg(t::G))
                .into_any_element()
        } else {
            Button::new("send")
                .icon(IconName::ArrowUp)
                .small()
                .w(px(24.))
                .h(px(24.))
                .when(!empty, |b| b.primary())
                .disabled(empty)
                .tooltip("Send")
                .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)))
                .into_any_element()
        };
        let state = self.state.clone();
        div()
            .relative()
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
                    .relative()
                    .gap(px(4.))
                    .pt(px(8.))
                    .px(px(8.))
                    .pb(px(6.))
                    .rounded(t::RADIUS)
                    .border_1()
                    .border_color(if focused { t::AC } else { t::LN })
                    .bg(t::G)
                    .when(focused, |el| el.shadow(t::field_ring()))
                    .when(!focused, |el| el.hover(|s| s.border_color(t::LN_HV)))
                    .children(self.render_attachments(cx))
                    .child(
                        Textarea::new(&self.input)
                            .bare()
                            .min_h(px(36.))
                            .line_height(px(20.))
                            .px(px(2.))
                            .token(move |token, _, cx| {
                                let text = token.token().text();
                                let mark = RefKind::of_token(text)
                                    .map(|k| k.mark(state.read(cx).bus.project(), text));
                                chip(mark, token.token().label().clone())
                                    .h(token.line_height().min(px(18.)))
                                    .max_w(token.available_width())
                                    .when(token.is_selected(), |el| {
                                        el.bg(t::alpha(t::AC, 0.3)).border_color(t::AC)
                                    })
                                    .when(token.is_disabled(), |el| el.opacity(0.5))
                            }),
                    )
                    .child(
                        h_flex()
                            .gap(px(2.))
                            .child(
                                Button::new("chat-at")
                                    .icon(IconName::AtSign)
                                    .ghost()
                                    .small()
                                    .tooltip("Reference")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.start_reference(window, cx)
                                    })),
                            )
                            .child(
                                Button::new("chat-attach")
                                    .icon(IconName::Paperclip)
                                    .ghost()
                                    .small()
                                    .disabled(busy)
                                    .tooltip("Attach files or folders")
                                    .on_click(cx.listener(|this, _, _, cx| this.prompt_attach(cx))),
                            )
                            .child(self.render_model_pickers(cx))
                            .child(div().flex_1())
                            .child(self.render_style_picker(cx))
                            .child(action),
                    )
                    .when(cx.has_active_drag(), |el| {
                        el.child(self.render_drop_target(cx))
                    }),
            )
            .children(self.render_suggestions(cx))
    }
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

/// The quiet title over a group of suggestions.
fn group_title(title: impl Into<SharedString>) -> Div {
    div()
        .px(px(8.))
        .pt(px(2.))
        .pb(px(3.))
        .text_size(px(10.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(t::T3)
        .child(title.into())
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
            flikk_core::command::Edge::Start => "in",
            flikk_core::command::Edge::End => "out",
        };
        let token = format!("@transition {clip}/{side}");
        let resolved = flikk_core::query::resolve_project_references(project, &token);
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
