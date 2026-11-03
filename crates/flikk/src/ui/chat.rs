//! Agent chat, docked in every workspace (PRD AG1–AG10): the thread with
//! streaming replies, tool calls and choice chips, the change-set and
//! approval cards fixed above the composer, and the quick-command bar.

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
use crate::ui::kit::{
    Button, DialogExt as _, Icon, IconName, Kbd, Panel, PanelHeader, ScrollableElement as _,
    Spinner, Tooltip, h_flex, theme as t, v_flex,
};
use composer::{Composer, ComposerEvent};
use flikk_core::agent::ChatRole;
use flikk_core::{ChangeItemId, ChangeSetId};
use gpui_kit::base::{TextView, TextViewStyle};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::HashSet;
use std::hash::{DefaultHasher, Hash, Hasher};

/// `text` with every reference token replaced by `@` and its label, for
/// places that show a message as plain text (chat titles).
pub(crate) fn plain_text(text: &str, project: &flikk_core::model::Project) -> String {
    refs::segments(text, project)
        .into_iter()
        .map(|s| match s {
            refs::Segment::Text(t) => t.to_owned(),
            refs::Segment::Ref(m) => format!("@{}", m.label),
        })
        .collect()
}

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
    /// The thread's rows, one per message, following the newest.
    list: ListState,
    reviews: change_set::Reviews,
    /// Change sets whose card is up: a resolved one stays, with what came of
    /// it, until the user's next message.
    shown: HashSet<ChangeSetId>,
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
        let list = ListState::new(count, ListAlignment::Top, px(400.));
        list.set_follow_mode(FollowMode::Tail);
        // The jump-to-latest button follows the scroll position.
        let this = cx.weak_entity();
        list.set_scroll_handler(move |_, _, cx| {
            let this = this.clone();
            cx.defer(move |cx| {
                let _ = this.update(cx, |_, cx| cx.notify());
            });
        });
        let composer = cx.new(|cx| Composer::new(state.clone(), window, cx));
        quick::register_composer(&composer, cx);
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
            list,
            reviews: Default::default(),
            shown: HashSet::new(),
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
        self.shown.clear();
        self.state.update(cx, |s, cx| {
            let block =
                context::references_block(s.bus.project(), s.services.analysis.as_deref(), &text);
            s.send_chat_with_context(text, block, cx)
        });
        self.scroll_to_end(cx);
    }

    /// Show the newest message and keep following the thread.
    fn scroll_to_end(&mut self, cx: &mut Context<Self>) {
        self.list.set_follow_mode(FollowMode::Tail);
        self.list.scroll_to_end();
        cx.notify();
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
            self.shown.clear();
        }
        let n = self.list.item_count();
        if switched || count < n {
            self.list.reset(count);
            self.scroll_to_end(cx);
        } else if count > n {
            self.list.splice(n..n, count - n);
            // The row before may have changed with it (choices answered).
            if n > 0 {
                self.list.remeasure_items(n - 1..n);
            }
        } else if changed {
            self.list.remeasure();
        }
    }

    /// The timeline picked a change item: open its card's review list and
    /// highlight the item there.
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
        let review = self.reviews.entry(set).or_default();
        review.open = true;
        review.reveal = Some(item);
    }

    fn remeasure(&mut self, cx: &mut Context<Self>) {
        self.list.remeasure();
        cx.notify();
    }

    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.dispatch_action(Box::new(crate::app::OpenAgentSettings), cx);
    }

    /// Ask before deleting a chat: its conversation can't be brought back.
    fn confirm_delete(state: Entity<EditorState>, id: String, window: &mut Window, cx: &mut App) {
        window.open_confirm(cx, move |dialog, _, _| {
            let (state, id) = (state.clone(), id.clone());
            dialog
                .title("Delete this chat?")
                .destructive()
                .confirm("Delete", move |_, cx| {
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
        let rows = sessions.into_iter().enumerate().map(|(ix, (id, title))| {
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
                .relative()
                .flex_none()
                .h(px(26.))
                .pl(px(8.))
                .pr(px(3.))
                .gap(px(8.))
                .rounded(t::RADIUS)
                .cursor_pointer()
                .map(|el| {
                    if current {
                        el.bg(t::alpha(t::AC, 0.1)).child(
                            div()
                                .absolute()
                                .left_0()
                                .top_0()
                                .bottom_0()
                                .w(px(2.))
                                .rounded_l(t::RADIUS)
                                .bg(t::AC),
                        )
                    } else {
                        el.text_color(t::T2)
                            .hover(|el| el.bg(t::HV).text_color(t::T1))
                    }
                })
                .on_click(open)
                .child(div().flex_1().min_w_0().truncate().child(title))
                .child(
                    div()
                        .flex_none()
                        .invisible()
                        .group_hover("chat-session", |el| el.visible())
                        .child(
                            Button::new(("chat-delete", ix))
                                .icon(IconName::Trash)
                                .ghost()
                                .xsmall()
                                .tooltip("Delete chat")
                                .on_click(delete),
                        ),
                )
        });
        div()
            .flex_1()
            .min_h_0()
            .child(
                v_flex()
                    .p(px(4.))
                    .gap(px(1.))
                    .children(rows)
                    .overflow_y_scrollbar(),
            )
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
        PanelHeader::new()
            .leading(
                h_flex()
                    .min_w_0()
                    .overflow_hidden()
                    .gap(px(6.))
                    .child(
                        div()
                            .id("chat-backend")
                            .flex_none()
                            .child(Icon::new(IconName::MessageSquareText).size(px(13.)))
                            .tooltip(Tooltip::text(backend)),
                    )
                    .child(div().flex_none().text_color(t::T1).child("Agent"))
                    .child(div().flex_none().text_color(t::T3).child("·"))
                    .child(div().min_w_0().truncate().child(title)),
            )
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
                    .icon(IconName::List)
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
                    .icon(IconName::Settings2)
                    .ghost()
                    .small()
                    .tooltip("Agent settings")
                    .on_click(cx.listener(|this, _, window, cx| this.open_settings(window, cx))),
            )
            .child(
                Button::new("chat-hide")
                    .icon(IconName::PanelRightClose)
                    .ghost()
                    .small()
                    .tooltip("Hide agent")
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(crate::app::ToggleAgent), cx)
                    }),
            )
    }

    fn render_thread(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.state.read(cx).chat.is_empty() {
            return h_flex()
                .flex_1()
                .justify_center()
                .gap(px(8.))
                .text_color(t::T3)
                .child("Ask for an edit")
                .child(Kbd::new("Cmd K"))
                .into_any_element();
        }
        let scrolled_up = self.list.max_offset_for_scrollbar().y > px(0.)
            && !self.list.is_following_tail()
            && !self.list.is_scrolled_to_end().unwrap_or(false);
        let this = cx.entity().downgrade();
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .child(
                list(self.list.clone(), move |ix, _, cx| {
                    this.update(cx, |this, cx| {
                        div()
                            .w_full()
                            .min_w_0()
                            .px(px(12.))
                            .pt(px(12.))
                            .child(this.render_message(ix, cx))
                            .into_any_element()
                    })
                    .unwrap_or_else(|_| div().into_any_element())
                })
                .size_full()
                .pb(px(14.)),
            )
            .vertical_scrollbar(&self.list)
            .when(scrolled_up, |el| {
                el.child(
                    h_flex()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom(px(10.))
                        .justify_center()
                        .child(
                            Button::new("chat-latest")
                                .icon(IconName::ArrowDown)
                                .tooltip("Jump to latest")
                                .rounded_full()
                                .on_click(cx.listener(|this, _, _, cx| this.scroll_to_end(cx))),
                        ),
                )
            })
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
        let busy = state.agent_busy();
        if msg.role == ChatRole::User {
            return h_flex()
                .w_full()
                .justify_end()
                .child(
                    div()
                        .max_w(relative(0.9))
                        .px(px(10.))
                        .py(px(7.))
                        .rounded_t(t::RADIUS_LG)
                        .rounded_bl(t::RADIUS_LG)
                        .rounded_br(px(3.))
                        .border_1()
                        .border_color(t::LN)
                        .bg(t::R1)
                        .line_height(px(20.))
                        .child(chips::message_text(ix, &msg.text, &self.state, cx)),
                )
                .into_any_element();
        }
        let text = refs::display(&msg.text, state.bus.project());
        let mut content = v_flex().w_full().gap(px(8.));
        if !text.is_empty() {
            let state = self.state.downgrade();
            content = content.child(
                div().w_full().line_height(px(19.)).child(
                    TextView::markdown(("chat-text", ix), text)
                        .style(text_style())
                        .on_link_click(move |url, _, _, cx| match refs::token_of_link(url) {
                            Some(token) => {
                                if let Some(state) = state.upgrade() {
                                    reveal::reveal(&state, &token, cx);
                                }
                            }
                            None => cx.open_url(url),
                        }),
                ),
            );
        }
        if !msg.choices.is_empty() {
            content = content.child(h_flex().flex_wrap().gap(px(6.)).children(
                msg.choices.iter().enumerate().map(|(i, choice)| {
                    let picked = answer.as_deref() == Some(choice.as_str());
                    let button = Button::new(("choice", ix * 16 + i));
                    if picked {
                        // The pick stays, marked; nothing more to press.
                        return button
                            .border_color(t::AC)
                            .bg(t::alpha(t::AC, 0.1))
                            .child(Icon::new(IconName::Check).size(px(12.)).color(t::AC))
                            .child(choice.clone());
                    }
                    let reply = choice.clone();
                    button
                        .label(choice.clone())
                        .disabled(answer.is_some() || busy)
                        .on_click(cx.listener(move |this, _, _, cx| this.send(reply.clone(), cx)))
                }),
            ));
        }
        if !msg.tools.is_empty() {
            content = content.child(self.render_tools(ix, &msg.tools, cx));
        }
        content.into_any_element()
    }

    fn render_footer(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let changes = self.render_change_sets(cx);
        let approvals = self.render_approvals(cx);
        let state = self.state.read(cx);
        let busy = state.agent_busy();
        let status = state.status.clone();
        v_flex()
            .flex_none()
            .gap(px(6.))
            .px(px(10.))
            .pb(px(10.))
            .children(changes)
            .children(approvals)
            .when(busy, |el| {
                el.child(
                    h_flex()
                        .h(px(20.))
                        .px(px(4.))
                        .gap(px(8.))
                        .text_size(px(11.))
                        .text_color(t::T2)
                        .child(Spinner::new().size(px(10.)))
                        .child("Working"),
                )
            })
            .when_some(status, |el, status| {
                el.child(
                    h_flex()
                        .items_start()
                        .gap(px(8.))
                        .px(px(10.))
                        .py(px(6.))
                        .rounded(t::RADIUS)
                        .border_1()
                        .border_color(t::alpha(t::ERR, 0.2))
                        .bg(t::alpha(t::ERR, 0.07))
                        .text_size(px(11.))
                        .text_color(t::ERR)
                        .child(
                            div()
                                .flex_none()
                                .pt(px(1.))
                                .child(Icon::new(IconName::CircleAlert).size(px(13.))),
                        )
                        .child(div().flex_1().min_w_0().line_clamp(3).child(status)),
                )
            })
            .child(self.composer.clone())
    }
}

impl Render for ChatPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.composer.read(cx).has_focus(window, cx);
        Panel::new()
            .focused(focused)
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

/// Agent replies: body text, inline code and fenced blocks on Flikk
/// surfaces, references as plain underlined text.
fn text_style() -> TextViewStyle {
    TextViewStyle::default()
        .with_foreground(t::T1.into())
        .with_muted_foreground(t::T2.into())
        .with_link(t::T1.into())
        .with_selection(t::alpha(t::AC, 0.3).into())
        .with_code_background(t::G.into())
        .with_border(t::LN.into())
        .with_paragraph_gap(rems(0.5))
        .with_inline_code(HighlightStyle {
            background_color: Some(t::R1.into()),
            ..Default::default()
        })
        .with_dark(true)
}

/// Changes whenever a visible row's height may have: streamed text, card
/// states, previews.
fn signature(s: &EditorState) -> u64 {
    let mut h = DefaultHasher::new();
    s.active_chat().hash(&mut h);
    s.chat.len().hash(&mut h);
    s.chat
        .last()
        .map(|m| {
            let running = m.tools.iter().filter(|t| t.running.is_some()).count();
            (m.text.len(), m.tools.len(), running)
        })
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
