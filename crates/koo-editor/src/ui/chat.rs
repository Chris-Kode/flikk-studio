//! Agent chat, docked in every workspace (PRD AG1–AG10): the thread with
//! streaming replies, tool calls, choice chips and change-set cards, the
//! composer and the quick-command bar.

mod approval;
mod change_set;
mod chips;
mod composer;
mod context;
mod quick;
mod recipes;
mod refs;
mod reveal;
mod tools;

use crate::state::EditorState;
use crate::ui::settings::SettingsPanel;
use crate::ui::timeline::glyph::{Glyph, glyph};
use composer::{Composer, ComposerEvent};
use gpui_kit::component::bubble::{Bubble, BubbleVariant};
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::message::{Message, MessageAlignment, MessageContent};
use gpui_kit::component::message_scroller::{MessageScroller, MessageScrollerState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::text::TextView;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Selectable as _, Sizable as _,
    WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::agent::ChatRole;
use koo_core::{ChangeItemId, ChangeSetId};
use std::collections::HashSet;

/// `text` with every reference token replaced by `@` and its label, for
/// places that show a message as plain text (chat titles).
pub(crate) fn plain_text(text: &str, project: &koo_core::model::Project) -> String {
    refs::segments(text, project)
        .into_iter()
        .map(|s| match s {
            refs::Segment::Text(t) => t.to_owned(),
            refs::Segment::Ref(m) => format!("@{}", m.label),
        })
        .collect()
}
use std::hash::{DefaultHasher, Hash, Hasher};

/// Key bindings for the chat: the composer's suggestion keys and the global
/// quick-command bar (cmd/ctrl-K). Call after `gpui_kit::init`; repeated
/// calls do nothing.
pub fn init(cx: &mut App) {
    if cx.has_global::<Initialized>() {
        return;
    }
    cx.set_global(Initialized);
    composer::init(cx);
    quick::init(cx);
}

struct Initialized;

impl Global for Initialized {}

pub struct ChatPanel {
    state: Entity<EditorState>,
    composer: Entity<Composer>,
    scroller: Entity<MessageScrollerState>,
    settings: Entity<SettingsPanel>,
    reviews: change_set::Reviews,
    /// Messages whose stack of tool calls is open.
    tools_open: HashSet<usize>,
    /// Tool calls (message, call) showing their details.
    tool_open: HashSet<(usize, usize)>,
    /// What the rows were last measured for.
    signature: u64,
    /// Chat session the rows show.
    chat: String,
    /// The chat sessions show in place of the thread.
    sessions_open: bool,
    /// Change item last revealed for the timeline.
    revealed: Option<(ChangeSetId, ChangeItemId)>,
    render_poll: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl ChatPanel {
    pub fn new(state: Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        init(cx);
        quick::register(&state, cx);
        let count = state.read(cx).chat.len();
        let scroller = cx.new(|cx| MessageScrollerState::new(count, cx));
        let composer = cx.new(|cx| Composer::new(state.clone(), window, cx));
        quick::register_composer(&composer, cx);
        let settings = cx.new(|cx| SettingsPanel::new(state.clone(), window, cx));
        let _subscriptions = vec![
            cx.observe(&state, |this, _, cx| {
                this.sync_rows(cx);
                this.reveal_focused(cx);
                this.poll_renders(cx);
                cx.notify();
            }),
            cx.subscribe(&composer, |this, _, event, cx| match event {
                ComposerEvent::Submit(text) => this.send(text.clone(), cx),
            }),
        ];
        let signature = signature(state.read(cx));
        let chat = state.read(cx).active_chat().to_owned();
        Self {
            state,
            composer,
            scroller,
            settings,
            reviews: Default::default(),
            tools_open: HashSet::new(),
            tool_open: HashSet::new(),
            signature,
            chat,
            sessions_open: false,
            revealed: None,
            render_poll: None,
            _subscriptions,
        }
    }

    /// Send a message; the agent also gets its references resolved.
    fn send(&mut self, text: String, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            let block =
                context::references_block(s.bus.project(), s.services.analysis.as_deref(), &text);
            s.send_chat_with_context(text, block, cx)
        });
        self.scroller.update(cx, |s, cx| s.scroll_to_end(cx));
    }

    /// Keep the virtual list's rows in step with the conversation: append
    /// new messages, remeasure when streaming text or a card changed.
    fn sync_rows(&mut self, cx: &mut Context<Self>) {
        let (count, sig, chat) = {
            let s = self.state.read(cx);
            (s.chat.len(), signature(s), s.active_chat().to_owned())
        };
        let changed = sig != self.signature;
        self.signature = sig;
        let switched = chat != self.chat;
        self.chat = chat;
        if switched {
            self.tools_open.clear();
            self.tool_open.clear();
        }
        self.scroller.update(cx, |list, cx| {
            let n = list.item_count();
            if switched {
                list.reset(count, cx);
                list.scroll_to_end(cx);
            } else if count < n {
                list.reset(count, cx);
            } else if count > n {
                list.append(count - n, cx);
            } else if changed {
                list.remeasure(cx);
            }
        });
    }

    /// The timeline picked a change item: scroll to its card, open the
    /// review list and highlight the item there.
    fn reveal_focused(&mut self, cx: &mut Context<Self>) {
        let focused = self.state.read(cx).focused_change_item();
        if focused == self.revealed {
            return;
        }
        self.revealed = focused;
        let Some((set, item)) = focused.filter(|_| self.state.read(cx).focused_change().is_some())
        else {
            return;
        };
        let Some(ix) = self
            .state
            .read(cx)
            .chat
            .iter()
            .rposition(|m| m.change_set == Some(set))
        else {
            return;
        };
        let review = self.reviews.entry(set).or_default();
        review.open = true;
        review.reveal = Some(item);
        self.scroller.update(cx, |list, cx| {
            list.remeasure(cx);
            list.scroll_to_item(ix, cx);
        });
    }

    fn remeasure(&mut self, cx: &mut Context<Self>) {
        self.scroller.update(cx, |list, cx| list.remeasure(cx));
        cx.notify();
    }

    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.settings.clone();
        window.open_sheet(cx, move |sheet, _, _| {
            sheet.title("Agent").size(px(380.)).child(panel.clone())
        });
    }

    /// Ask before deleting a chat: its conversation can't be brought back.
    fn confirm_delete(state: Entity<EditorState>, id: String, window: &mut Window, cx: &mut App) {
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (state, id) = (state.clone(), id.clone());
            alert
                .title("Delete this chat?")
                .ok_text("Delete")
                .ok_variant(ButtonVariant::Danger)
                .show_cancel(true)
                .on_ok(move |_, _, cx| {
                    state.update(cx, |s, cx| s.delete_chat(&id, cx));
                    true
                })
        });
    }

    /// Chat sessions in place of the thread, most recent first: pick one to
    /// open it, hover one to delete it.
    fn render_sessions(&self, cx: &mut Context<Self>) -> AnyElement {
        let s = self.state.read(cx);
        let active = s.active_chat().to_owned();
        let sessions = s.chat_sessions();
        let theme = cx.theme();
        let (accent, hover, muted) = (theme.accent, theme.secondary_hover, theme.muted_foreground);
        v_flex()
            .id("chat-sessions")
            .flex_1()
            .min_h_0()
            .p_2()
            .gap_0p5()
            .overflow_y_scroll()
            .children(sessions.into_iter().enumerate().map(|(ix, (id, title))| {
                let current = id == active;
                let open = {
                    let id = id.clone();
                    cx.listener(move |this, _, _, cx| {
                        this.sessions_open = false;
                        this.state.update(cx, |s, cx| s.switch_chat(&id, cx));
                        cx.notify();
                    })
                };
                let delete = cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    Self::confirm_delete(this.state.clone(), id.clone(), window, cx);
                });
                h_flex()
                    .id(("chat-session", ix))
                    .group("chat-session")
                    .flex_none()
                    .h(px(32.))
                    .pl_2()
                    .pr_1()
                    .gap_1()
                    .rounded_md()
                    .cursor_pointer()
                    .text_sm()
                    .when(current, |el| el.bg(accent))
                    .hover(|el| el.bg(hover))
                    .on_click(open)
                    .child(div().flex_1().min_w_0().truncate().child(title))
                    .child(
                        div()
                            .flex_none()
                            .invisible()
                            .group_hover("chat-session", |el| el.visible())
                            .child(
                                Button::new(("chat-delete", ix))
                                    .ghost()
                                    .xsmall()
                                    .tooltip("Delete chat")
                                    .child(glyph(Glyph::Trash, px(13.), muted))
                                    .on_click(delete),
                            ),
                    )
            }))
            .into_any_element()
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.state.read(cx);
        let empty = s.chat.is_empty();
        let busy = s.agent_busy();
        let backend = s.settings.backend.label();
        let title = s
            .chat_sessions()
            .into_iter()
            .find(|(id, _)| id == s.active_chat())
            .map_or_else(|| "Untitled".to_owned(), |(_, title)| title);
        let theme = cx.theme();
        h_flex()
            .h(px(40.))
            .flex_none()
            .px_3()
            .gap_2()
            .justify_between()
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .gap_1()
                    .child(
                        div()
                            .id("chat-backend")
                            .flex_none()
                            .child(
                                Icon::new(IconName::Bot)
                                    .size_4()
                                    .text_color(theme.muted_foreground),
                            )
                            .tooltip(move |window, cx| Tooltip::new(backend).build(window, cx)),
                    )
                    .child(div().min_w_0().text_sm().truncate().child(title)),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap_0p5()
                    .child(
                        Button::new("chat-new")
                            .icon(IconName::Plus)
                            .ghost()
                            .small()
                            .tooltip("New chat")
                            .disabled(empty || busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.sessions_open = false;
                                this.state.update(cx, |s, cx| s.new_chat(cx))
                            })),
                    )
                    .child(
                        Button::new("chat-sessions-toggle")
                            .icon(IconName::Menu)
                            .ghost()
                            .small()
                            .tooltip("Chats")
                            .selected(self.sessions_open)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.sessions_open = !this.sessions_open;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("chat-settings")
                            .icon(IconName::Settings)
                            .ghost()
                            .small()
                            .tooltip("Agent settings")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.open_settings(window, cx)),
                            ),
                    ),
            )
    }

    fn render_thread(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.state.read(cx).chat.is_empty() {
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("Ask for an edit")
                .into_any_element();
        }
        let this = cx.entity().downgrade();
        div()
            .flex_1()
            .min_h_0()
            .child(
                MessageScroller::new("chat-thread", self.scroller.clone(), move |ix, _, cx| {
                    this.update(cx, |this, cx| this.render_message(ix, cx))
                        .unwrap_or_else(|_| div().into_any_element())
                })
                .with_bottom_fade(cx.theme().background)
                .with_row_style(StyleRefinement::default().pb_5()),
            )
            .into_any_element()
    }

    fn render_message(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let state = self.state.read(cx);
        let Some(msg) = state.chat.get(ix).cloned() else {
            return div().into_any_element();
        };
        let answer = state
            .chat
            .get(ix + 1)
            .filter(|m| m.role == ChatRole::User)
            .map(|m| m.text.clone());
        let text = match msg.role {
            ChatRole::User => String::new(),
            ChatRole::Agent => refs::display(&msg.text, state.bus.project()),
        };
        let busy = state.agent_busy();
        match msg.role {
            ChatRole::User => {
                Message::new()
                    .alignment(MessageAlignment::End)
                    .content(MessageContent::new().bubble(
                        Bubble::new().with_variant(BubbleVariant::Muted).child(
                            div().text_sm().child(chips::message_text(
                                ix,
                                &msg.text,
                                &self.state,
                                cx,
                            )),
                        ),
                    ))
                    .into_any_element()
            }
            ChatRole::Agent => {
                let mut content = MessageContent::new().gap_2();
                if !text.is_empty() {
                    let state = self.state.downgrade();
                    content = content.child(div().w_full().text_sm().child(
                        TextView::markdown(("chat-text", ix), text).on_link_click(
                            move |url, _, _, cx| match refs::token_of_link(url) {
                                Some(token) => {
                                    if let Some(state) = state.upgrade() {
                                        reveal::reveal(&state, &token, cx);
                                    }
                                }
                                None => cx.open_url(url),
                            },
                        ),
                    ));
                }
                if !msg.choices.is_empty() {
                    content = content.child(h_flex().flex_wrap().gap_1p5().children(
                        msg.choices.iter().enumerate().map(|(i, choice)| {
                            let picked = answer.as_deref() == Some(choice.as_str());
                            let reply = choice.clone();
                            Button::new(("choice", ix * 16 + i))
                                .label(choice.clone())
                                .small()
                                .outline()
                                .selected(picked)
                                .disabled(answer.is_some() || busy)
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.send(reply.clone(), cx)),
                                )
                        }),
                    ));
                }
                if !msg.tools.is_empty() {
                    content = content.child(self.render_tools(ix, &msg.tools, cx));
                }
                if let Some(set) = msg.change_set {
                    content = content.child(self.render_change_set(set, cx));
                }
                Message::new().content(content).into_any_element()
            }
        }
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let busy = state.agent_busy();
        let status = state.status.clone();
        let approvals = self.render_approvals(cx);
        let theme = cx.theme();
        v_flex()
            .flex_none()
            .children(approvals)
            .when(busy, |el| {
                el.child(
                    h_flex()
                        .px_4()
                        .pt_1()
                        .gap_2()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(Spinner::new().xsmall())
                        .child("Working"),
                )
            })
            .when_some(status, |el, status| {
                el.child(
                    h_flex()
                        .px_4()
                        .pt_1()
                        .gap_1p5()
                        .text_xs()
                        .text_color(theme.red)
                        .child(Icon::new(IconName::CircleAlert).size_3().flex_none())
                        .child(div().min_w_0().truncate().child(status)),
                )
            })
            .child(self.composer.clone())
    }
}

impl Render for ChatPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(self.render_header(cx))
            .map(|el| {
                if self.sessions_open {
                    el.child(self.render_sessions(cx))
                } else {
                    el.child(self.render_thread(cx))
                        .child(self.render_footer(cx))
                }
            })
    }
}

/// Changes whenever a visible row's height may have: streamed text, card
/// states, previews.
fn signature(s: &EditorState) -> u64 {
    let mut h = DefaultHasher::new();
    s.active_chat().hash(&mut h);
    s.chat.len().hash(&mut h);
    s.chat
        .last()
        .map(|m| (m.text.len(), m.tools.len()))
        .hash(&mut h);
    s.preview.hash(&mut h);
    s.agent_busy().hash(&mut h);
    s.focused_change_item().hash(&mut h);
    for set in s.bus.change_sets() {
        (set.state as u8).hash(&mut h);
        for item in &set.items {
            (item.state as u8).hash(&mut h);
        }
    }
    h.finish()
}
