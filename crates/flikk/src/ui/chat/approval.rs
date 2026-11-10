//! Approval cards (PRD: project/memory and deliver tools ask first), above
//! the composer: one row per request with Deny / Approve, then what came of
//! it in its place until the user's next message. Answered requests stack
//! into one card with a tally once there is more than one, and both lists
//! scroll past a few rows, so a run of generations never crowds out the
//! thread. A voice to be added or read with plays from its card; approving a
//! voice design opens the Voices dialog, where the user hears the takes and
//! keeps one.

use super::ChatPanel;
use crate::state::voices::{VoiceSample, approval_voice};
use crate::state::{ApprovalOutcome, EditorState};
use crate::ui::kit::{
    Button, Icon, IconName, Progress, ScrollableElement as _, Spinner, Tooltip, h_flex, theme as t,
    v_flex,
};
use crate::ui::listen::listen_dot;
use crate::ui::voices::{self, OnKept, Start};
use flikk_agent::PendingApproval;
use flikk_agent::tools::ApprovalRequest;
use flikk_core::cloud::Capability;
use flikk_core::command::ToolCategory;
use flikk_deliver::RenderState;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::Value;
use std::rc::Rc;
use std::time::Duration;

const RENDER_POLL: Duration = Duration::from_millis(250);
/// Tallest the waiting cards get before they scroll: five cards.
const PENDING_MAX: Pixels = px(204.);
/// Tallest they get with a video's card among them, which shows what the
/// video is made from.
const PENDING_MAX_SHOT: Pixels = px(300.);
/// Tallest the answered stack's list gets before it scrolls: four and a
/// half rows, so the cut-off row shows there is more.
const SETTLED_MAX: Pixels = px(162.);

impl ChatPanel {
    /// Keep a card for each video waiting on the user, and none for those
    /// they answered.
    pub(super) fn sync_shots(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pending = self.state.read(cx).pending_approvals();
        self.shots
            .retain(|id, _| pending.iter().any(|p| p.id == *id));
        for approval in pending {
            if approval.request.tool != "generate_video" || self.shots.contains_key(&approval.id) {
                continue;
            }
            let Some(shot) = flikk_agent::tools::video::from_json(&approval.request.args) else {
                continue;
            };
            let state = self.state.clone();
            let card =
                cx.new(|cx| super::shot::ShotCard::new(state, approval.id, shot, window, cx));
            if crate::demo::shot::asked().as_deref() == Some("blockout") {
                let card = card.clone();
                cx.defer_in(window, move |_, window, cx| {
                    card.update(cx, |card, cx| card.edit_blockout(window, cx))
                });
            }
            self.shots.insert(approval.id, card);
        }
    }

    pub(super) fn render_approvals(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let state = self.state.read(cx);
        let settled = state.settled.clone();
        let pending = state.pending_approvals().to_vec();
        let mut cards = Vec::new();
        match settled.as_slice() {
            [] => {}
            [(p, o)] => cards.push(settled_row(p, o, state, true).into_any_element()),
            many => cards.push(self.render_settled_stack(many, cx)),
        }
        if !pending.is_empty() {
            let rows = pending.iter().map(|p| self.render_approval(p, cx));
            cards.push(
                div()
                    .relative()
                    .child(
                        v_flex()
                            .id("approvals-pending")
                            .max_h(if self.shots.is_empty() {
                                PENDING_MAX
                            } else {
                                PENDING_MAX_SHOT
                            })
                            .gap(px(6.))
                            .children(rows)
                            .track_scroll(&self.pending_scroll)
                            .overflow_y_scroll(),
                    )
                    .vertical_scrollbar(&self.pending_scroll)
                    .into_any_element(),
            );
        }
        cards
    }

    /// The answered requests as one card: how they stand and what they
    /// make in its header, their rows in a list that scrolls.
    fn render_settled_stack(
        &self,
        settled: &[(PendingApproval, ApprovalOutcome)],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.state.read(cx);
        let open = !self.approvals_folded;
        let phases: Vec<Phase> = settled.iter().map(|(_, o)| phase(o, state)).collect();
        let count = |p: Phase| phases.iter().filter(|q| **q == p).count();
        let (running, failed, finished) = (
            count(Phase::Running),
            count(Phase::Failed),
            phases.len() - count(Phase::Running),
        );
        let (status, label, color) = if running > 0 {
            (
                Spinner::new().color(t::T2).into_any_element(),
                "Working".to_owned(),
                t::T1,
            )
        } else if failed > 0 {
            (
                Icon::new(IconName::CircleAlert)
                    .color(t::ERR)
                    .into_any_element(),
                format!("{failed} failed"),
                t::ERR,
            )
        } else {
            (
                Icon::new(IconName::CircleCheck)
                    .color(t::OK)
                    .into_any_element(),
                "Done".to_owned(),
                t::T1,
            )
        };
        // What the requests make, one icon per kind with how many.
        let mut kinds: Vec<(IconName, usize)> = Vec::new();
        for (p, _) in settled {
            let icon = request_icon(&p.request);
            match kinds.iter_mut().find(|(k, _)| *k == icon) {
                Some((_, n)) => *n += 1,
                None => kinds.push((icon, 1)),
            }
        }
        let rows: Option<Vec<_>> = open.then(|| {
            settled
                .iter()
                .map(|(p, o)| {
                    div()
                        .border_t_1()
                        .border_color(t::LN)
                        .child(settled_row(p, o, state, false))
                })
                .collect()
        });
        let head = h_flex()
            .id("approvals-head")
            .h(px(36.))
            .pl(px(10.))
            .pr(px(8.))
            .gap(px(8.))
            .cursor_pointer()
            .hover(|el| el.bg(t::HV))
            .child(status)
            .child(div().flex_none().text_color(color).child(label))
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .gap(px(10.))
                    .children(kinds.into_iter().map(|(icon, n)| {
                        h_flex()
                            .flex_none()
                            .gap(px(4.))
                            .text_color(t::T2)
                            .child(Icon::new(icon).size(px(12.)))
                            .child(
                                div()
                                    .font_family(t::mono())
                                    .text_size(px(11.))
                                    .child(n.to_string()),
                            )
                    })),
            )
            .child(
                div()
                    .flex_none()
                    .font_family(t::mono())
                    .text_size(px(11.))
                    .text_color(t::T3)
                    .child(format!("{finished}/{}", phases.len())),
            )
            .child(
                Icon::new(if open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .size(px(12.))
                .color(t::T3),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.approvals_folded = !this.approvals_folded;
                cx.notify();
            }));
        v_flex()
            .id("approvals-settled")
            .flex_none()
            .w_full()
            .overflow_hidden()
            .rounded(t::RADIUS)
            .border_1()
            .border_color(if failed > 0 && running == 0 {
                t::alpha(t::ERR, 0.25)
            } else {
                t::LN
            })
            .bg(t::R1)
            .child(head)
            .when_some(rows, |el, rows| {
                el.child(
                    div()
                        .relative()
                        .child(
                            v_flex()
                                .id("approvals-settled-rows")
                                .max_h(SETTLED_MAX)
                                .children(rows)
                                .track_scroll(&self.settled_scroll)
                                .overflow_y_scroll(),
                        )
                        .vertical_scrollbar(&self.settled_scroll),
                )
            })
            .into_any_element()
    }

    /// A request waiting on the user: what it does, Deny and Approve.
    fn render_approval(&self, approval: &PendingApproval, cx: &mut Context<Self>) -> AnyElement {
        let id = approval.id;
        let request = &approval.request;
        if let Some(card) = self.shots.get(&id) {
            return card.clone().into_any_element();
        }
        if super::entries::is_entry_tool(&request.tool)
            && let Some(card) = self.render_entry_approval(id, &request.tool, &request.args, cx)
        {
            return card;
        }
        let summary: SharedString = request.summary.clone().into();
        let approve = Button::new(("approve", id as usize))
            .label("Approve")
            .primary()
            .small();
        let approve =
            if request.tool == "design_voice" {
                let args = request.args.clone();
                approve.on_click(cx.listener(move |this, _, window, cx| {
                    this.design_in_dialog(id, &args, window, cx)
                }))
            } else {
                approve.on_click(cx.listener(move |this, _, _, cx| {
                    this.state.update(cx, |s, cx| s.approve(id, cx))
                }))
            };
        let leading = self.sample_button(request, id, cx).unwrap_or_else(|| {
            Icon::new(request_icon(request))
                .color(t::T2)
                .into_any_element()
        });
        card(("approval", id as usize), true)
            .border_color(t::LN)
            .child(leading)
            .child(
                div()
                    .id(("approval-text", id as usize))
                    .flex_1()
                    .min_w_0()
                    // Readable in full before deciding.
                    .line_clamp(2)
                    .child(summary.clone())
                    .tooltip(Tooltip::text(summary)),
            )
            .child(
                Button::new(("deny", id as usize))
                    .label("Deny")
                    .small()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.update(cx, |s, cx| s.deny_approval(id, cx))
                    })),
            )
            .child(approve)
            .into_any_element()
    }

    /// Play the voice `request` adds or reads with, when it can be heard.
    fn sample_button(
        &self,
        request: &ApprovalRequest,
        id: u64,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (voice, url) = approval_voice(request)?;
        let sample = self.state.read(cx).voice_sample(&voice, url.as_deref())?;
        let key = ("approval-listen", id as usize);
        Some(match sample {
            VoiceSample::Here(item) => {
                listen_dot(key, &self.state, &item, px(22.), cx).into_any_element()
            }
            VoiceSample::Fetching => h_flex()
                .flex_none()
                .size(px(22.))
                .justify_center()
                .child(Spinner::new().color(t::T2))
                .into_any_element(),
            VoiceSample::Remote(voice) => Button::new(key)
                .ghost()
                .small()
                .icon(IconName::Play)
                .tooltip("Play")
                .on_click(cx.listener(move |this, _, _, cx| {
                    let voice = voice.clone();
                    this.state.update(cx, |s, cx| s.hear_voice(voice, cx))
                }))
                .into_any_element(),
        })
    }

    /// Approving a voice design: the Voices dialog makes the takes for the
    /// user to hear; the request is settled with the voice they keep, and
    /// stays waiting when they close it without one.
    fn design_in_dialog(
        &mut self,
        id: u64,
        args: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((design, name)) = flikk_agent::tools::voices::design_from_json(args) else {
            return self.state.update(cx, |s, cx| s.approve(id, cx));
        };
        let state = self.state.clone();
        let on_kept: OnKept =
            Rc::new(move |voice, _, cx| state.update(cx, |s, cx| s.settle_design(id, voice, cx)));
        voices::open(
            self.state.clone(),
            Start::Design { design, name },
            Some(on_kept),
            window,
            cx,
        );
    }

    /// Repaint while a render approved here is running, so its progress moves.
    pub(super) fn poll_renders(&mut self, cx: &mut Context<Self>) {
        if self.render_poll.is_some() || !rendering(self.state.read(cx)) {
            return;
        }
        self.render_poll = Some(crate::ui::repaint_while(
            cx,
            RENDER_POLL,
            |this, cx| rendering(this.state.read(cx)),
            |this| this.render_poll = None,
        ));
    }
}

fn rendering(state: &EditorState) -> bool {
    state.settled.iter().any(|(_, outcome)| match outcome {
        ApprovalOutcome::Rendering { job, .. } => state
            .render_job(*job)
            .is_some_and(|j| !j.state.is_finished()),
        _ => false,
    })
}

/// The 36 px row an approval sits in: a card of its own when `framed`,
/// else a row inside the answered stack.
fn card(id: impl Into<ElementId>, framed: bool) -> Stateful<Div> {
    h_flex()
        .id(id)
        .flex_none()
        .min_h(px(36.))
        .pl(px(10.))
        .pr(px(8.))
        .py(px(4.))
        .gap(px(8.))
        .when(framed, |el| el.rounded(t::RADIUS).border_1().bg(t::R1))
}

/// What a request makes or does: a voice, a sound, a picture, a video, a
/// render, or something kept in memory.
fn request_icon(request: &ApprovalRequest) -> IconName {
    use flikk_agent::tools::{generate, image, upscale};
    match request.tool.as_str() {
        "generate_video" | upscale::TOOL => IconName::Video,
        image::TOOL => IconName::Image,
        "generate_audio" => match generate::from_json(&request.args).map(|(r, _)| r.capability()) {
            Some(Capability::Voiceover) => IconName::Mic,
            _ => IconName::Volume2,
        },
        "replace_words" | "add_voice" | "design_voice" => IconName::Mic,
        "save_skill" | "delete_skill" => IconName::SquareSlash,
        "save_style" | "delete_style" => IconName::Palette,
        _ => match request.category {
            ToolCategory::Deliver => IconName::Clapperboard,
            ToolCategory::Generate => IconName::Volume2,
            ToolCategory::CaptionStyles => IconName::Captions,
            _ => IconName::BookOpen,
        },
    }
}

/// Where an answered request stands, for the stack's tally.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Running,
    Done,
    Failed,
    /// Denied by the user, or a render they canceled.
    Stopped,
}

fn phase(outcome: &ApprovalOutcome, state: &EditorState) -> Phase {
    match outcome {
        ApprovalOutcome::Working => Phase::Running,
        ApprovalOutcome::Done(_) => Phase::Done,
        ApprovalOutcome::Failed(_) => Phase::Failed,
        ApprovalOutcome::Denied => Phase::Stopped,
        ApprovalOutcome::Rendering { job, .. } => match state.render_job(*job).map(|j| j.state) {
            Some(RenderState::Queued | RenderState::Running { .. }) => Phase::Running,
            Some(RenderState::Failed { .. }) => Phase::Failed,
            Some(RenderState::Canceled) => Phase::Stopped,
            Some(RenderState::Done) | None => Phase::Done,
        },
    }
}

/// A request the user answered: what it makes, what came of it and how it
/// stands, until their next message. The request itself is in the tooltip.
fn settled_row(
    approval: &PendingApproval,
    outcome: &ApprovalOutcome,
    state: &EditorState,
    framed: bool,
) -> Stateful<Div> {
    let id = ("approval", approval.id as usize);
    let summary: SharedString = approval.request.summary.clone().into();
    let icon = request_icon(&approval.request);
    let row = |color: Rgba| {
        card(id, framed)
            .border_color(t::LN)
            .text_color(color)
            .tooltip(Tooltip::text(summary.clone()))
            .child(Icon::new(icon).color(t::T2))
    };
    let text = |text: SharedString| div().flex_1().min_w_0().truncate().child(text);
    let done = |label: String| {
        row(t::T1)
            .child(text(label.into()))
            .child(Icon::new(IconName::CircleCheck).color(t::OK))
    };
    let failed = |message: String| {
        card(id, framed)
            .border_color(t::alpha(t::ERR, 0.25))
            .text_color(t::ERR)
            .tooltip(Tooltip::text(message.clone()))
            .child(Icon::new(icon))
            .child(text(format!("Failed · {message}").into()))
            .child(Icon::new(IconName::CircleAlert))
    };
    let stopped = |label: &'static str| {
        row(t::T2)
            .child(text(label.into()))
            .child(Icon::new(IconName::CircleX).color(t::T3))
    };
    match outcome {
        ApprovalOutcome::Working => row(t::T2)
            .child(text(summary.clone()))
            .child(Spinner::new().color(t::T2)),
        ApprovalOutcome::Done(label) => done(label.clone()),
        ApprovalOutcome::Denied => stopped("Denied"),
        ApprovalOutcome::Failed(message) => failed(message.clone()),
        ApprovalOutcome::Rendering { job, file } => match state.render_job(*job).map(|j| j.state) {
            Some(RenderState::Queued) => row(t::T2).child(div().flex_none().child("Queued")).child(
                div()
                    .flex_1()
                    .child(Progress::new().indeterminate().h(px(3.))),
            ),
            Some(RenderState::Running { fraction }) => row(t::T2)
                .child(
                    h_flex().flex_none().gap(px(4.)).child("Rendering").child(
                        div()
                            .font_family(t::mono())
                            .child(format!("{:.0} %", (fraction * 100.0).clamp(0.0, 100.0))),
                    ),
                )
                .child(
                    div()
                        .flex_1()
                        .child(Progress::new().value(fraction).h(px(3.))),
                ),
            Some(RenderState::Failed { message }) => failed(message),
            Some(RenderState::Canceled) => stopped("Canceled"),
            // The name it was written under: another file may have taken
            // the one it was queued with.
            Some(RenderState::Done) => done(state.render_job(*job).map_or_else(
                || file.clone(),
                |j| crate::agent_link::approve::file_name(&j.output),
            )),
            None => done(file.clone()),
        },
    }
}
