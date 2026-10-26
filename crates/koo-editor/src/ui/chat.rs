//! Agent chat, docked in every workspace (PRD AG1–AG10): the thread with
//! streaming replies, choice chips and change-set cards, the composer, the
//! activity log and the quick-command bar.

mod activity;
mod change_set;
mod composer;
mod demo;
mod quick;
mod recipes;
mod refs;

use crate::state::EditorState;
use crate::ui::settings::SettingsPanel;
use composer::{Composer, ComposerEvent};
use gpui_kit::component::bubble::{Bubble, BubbleVariant};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::message::{Message, MessageAlignment, MessageContent};
use gpui_kit::component::message_scroller::{MessageScroller, MessageScrollerState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::text::TextView;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Selectable as _, Sizable as _,
    StyledExt as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::agent::ChatRole;
use std::collections::HashSet;
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
    show_activity: bool,
    reviews: change_set::Reviews,
    activity_open: HashSet<usize>,
    /// What the rows were last measured for.
    signature: u64,
    _subscriptions: Vec<Subscription>,
}

impl ChatPanel {
    pub fn new(state: Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        init(cx);
        if demo::enabled() {
            state.update(cx, |s, cx| {
                demo::seed(s);
                cx.notify();
            });
        }
        quick::register(&state, cx);
        let count = state.read(cx).chat.len();
        let scroller = cx.new(|cx| MessageScrollerState::new(count, cx));
        let composer = cx.new(|cx| Composer::new(state.clone(), window, cx));
        let settings = cx.new(|cx| SettingsPanel::new(state.clone(), window, cx));
        let _subscriptions = vec![
            cx.observe(&state, |this, _, cx| {
                this.sync_rows(cx);
                cx.notify();
            }),
            cx.subscribe(&composer, |this, _, event, cx| match event {
                ComposerEvent::Submit(text) => this.send(text.clone(), cx),
            }),
        ];
        let signature = signature(state.read(cx));
        Self {
            state,
            composer,
            scroller,
            settings,
            show_activity: false,
            reviews: Default::default(),
            activity_open: HashSet::new(),
            signature,
            _subscriptions,
        }
    }

    fn send(&mut self, text: String, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| s.send_chat(text, cx));
        self.scroller.update(cx, |s, cx| s.scroll_to_end(cx));
    }

    /// Keep the virtual list's rows in step with the conversation: append
    /// new messages, remeasure when streaming text or a card changed.
    fn sync_rows(&mut self, cx: &mut Context<Self>) {
        let (count, sig) = {
            let s = self.state.read(cx);
            (s.chat.len(), signature(s))
        };
        let changed = sig != self.signature;
        self.signature = sig;
        self.scroller.update(cx, |list, cx| {
            let n = list.item_count();
            if count < n {
                list.reset(count, cx);
            } else if count > n {
                list.append(count - n, cx);
            } else if changed {
                list.remeasure(cx);
            }
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

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let backend = self.state.read(cx).settings.backend.label();
        h_flex()
            .h(px(40.))
            .flex_none()
            .px_3()
            .justify_between()
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Icon::new(IconName::Bot)
                            .size_4()
                            .text_color(theme.muted_foreground),
                    )
                    .child(div().text_sm().font_medium().child(backend)),
            )
            .child(
                h_flex()
                    .gap_0p5()
                    .child(
                        Button::new("chat-activity")
                            .icon(IconName::GalleryVerticalEnd)
                            .ghost()
                            .small()
                            .selected(self.show_activity)
                            .tooltip("Activity")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_activity = !this.show_activity;
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
        let text = refs::display(&msg.text, state.bus.project());
        let busy = state.agent_busy();
        match msg.role {
            ChatRole::User => Message::new()
                .alignment(MessageAlignment::End)
                .content(
                    MessageContent::new().bubble(
                        Bubble::new()
                            .with_variant(BubbleVariant::Muted)
                            .child(TextView::markdown(
                                ("chat-text", ix),
                                // Keep the user's line breaks.
                                text.replace('\n', "  \n"),
                            )),
                    ),
                )
                .into_any_element(),
            ChatRole::Agent => {
                let mut content = MessageContent::new().gap_2();
                if !text.is_empty() {
                    content = content.child(
                        div()
                            .w_full()
                            .text_sm()
                            .child(TextView::markdown(("chat-text", ix), text)),
                    );
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
        let theme = cx.theme();
        v_flex()
            .flex_none()
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
        let body = if self.show_activity {
            self.render_activity(cx)
        } else {
            self.render_thread(cx)
        };
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(self.render_header(cx))
            .child(body)
            .when(!self.show_activity, |el| el.child(self.render_footer(cx)))
    }
}

/// Changes whenever a visible row's height may have: streamed text, card
/// states, previews.
fn signature(s: &EditorState) -> u64 {
    let mut h = DefaultHasher::new();
    s.chat.len().hash(&mut h);
    s.chat.last().map(|m| m.text.len()).hash(&mut h);
    s.preview.hash(&mut h);
    s.agent_busy().hash(&mut h);
    for set in s.bus.change_sets() {
        (set.state as u8).hash(&mut h);
        for item in &set.items {
            (item.state as u8).hash(&mut h);
        }
    }
    h.finish()
}
