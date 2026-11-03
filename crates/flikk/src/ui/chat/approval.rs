//! Approval cards (PRD: project/memory and deliver tools ask first), above
//! the composer: one row per request with Deny / Approve, then what came of
//! it in its place until the user's next message. A voice to be
//! added or read with plays from its card; approving a voice design opens
//! the Voices dialog, where the user hears the takes and keeps one.

use super::ChatPanel;
use crate::state::voices::{VoiceSample, approval_voice};
use crate::state::{ApprovalOutcome, EditorState};
use crate::ui::kit::{Button, Icon, IconName, Progress, Spinner, Tooltip, h_flex, theme as t};
use crate::ui::media_pool::listen_button;
use crate::ui::voices::{self, OnKept, Start};
use flikk_agent::PendingApproval;
use flikk_agent::tools::ApprovalRequest;
use flikk_core::command::ToolCategory;
use flikk_deliver::RenderState;
use gpui_kit::*;
use serde_json::Value;
use std::rc::Rc;
use std::time::Duration;

const RENDER_POLL: Duration = Duration::from_millis(250);

impl ChatPanel {
    pub(super) fn render_approvals(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let state = self.state.read(cx);
        let cards: Vec<(PendingApproval, Option<ApprovalOutcome>)> = state
            .settled
            .iter()
            .map(|(p, o)| (p.clone(), Some(o.clone())))
            .chain(state.pending_approvals().iter().map(|p| (p.clone(), None)))
            .collect();
        cards
            .iter()
            .map(|(p, o)| match o {
                None => self.render_approval(p, cx),
                Some(o) => settled(p, o, self.state.read(cx)),
            })
            .collect()
    }

    /// A request waiting on the user: what it does, Deny and Approve.
    fn render_approval(&self, approval: &PendingApproval, cx: &mut Context<Self>) -> AnyElement {
        let id = approval.id;
        let request = &approval.request;
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
            Icon::new(category_icon(request))
                .color(t::T2)
                .into_any_element()
        });
        card(("approval", id as usize))
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
                listen_button(key, &self.state, &item, cx).into_any_element()
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
        self.render_poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(RENDER_POLL).await;
                let running = this.update(cx, |this, cx| {
                    cx.notify();
                    rendering(this.state.read(cx))
                });
                if !matches!(running, Ok(true)) {
                    break;
                }
            }
            let _ = this.update(cx, |this, _| this.render_poll = None);
        }));
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

/// The 36 px row an approval sits in.
fn card(id: impl Into<ElementId>) -> Stateful<Div> {
    h_flex()
        .id(id)
        .flex_none()
        .min_h(px(36.))
        .pl(px(10.))
        .pr(px(6.))
        .py(px(4.))
        .gap(px(8.))
        .rounded(t::RADIUS)
        .border_1()
        .bg(t::R1)
}

fn category_icon(request: &ApprovalRequest) -> IconName {
    match request.category {
        ToolCategory::Deliver => IconName::Clapperboard,
        ToolCategory::Generate => IconName::Mic,
        _ => IconName::BookOpen,
    }
}

/// A request the user answered: what came of it, until their next message.
/// The request itself is in the tooltip.
fn settled(
    approval: &PendingApproval,
    outcome: &ApprovalOutcome,
    state: &EditorState,
) -> AnyElement {
    let id = approval.id as usize;
    let summary: SharedString = approval.request.summary.clone().into();
    let icon = category_icon(&approval.request);
    let row = card(("approval", id))
        .border_color(t::LN)
        .text_color(t::T2)
        .tooltip(Tooltip::text(summary.clone()));
    let text = |text: SharedString| div().flex_1().min_w_0().truncate().child(text);
    let done = |label: String| {
        card(("approval", id))
            .border_color(t::LN)
            .tooltip(Tooltip::text(summary.clone()))
            .child(Icon::new(IconName::CircleCheck).color(t::OK))
            .child(text(label.into()))
            .into_any_element()
    };
    let failed = |message: String| {
        card(("approval", id))
            .border_color(t::alpha(t::ERR, 0.25))
            .text_color(t::ERR)
            .tooltip(Tooltip::text(message.clone()))
            .child(Icon::new(IconName::CircleAlert))
            .child(text(format!("Failed · {message}").into()))
            .into_any_element()
    };
    let note = |icon: IconName, label: &'static str| {
        row.child(Icon::new(icon))
            .child(text(label.into()))
            .into_any_element()
    };
    match outcome {
        ApprovalOutcome::Working => card(("approval", id))
            .border_color(t::LN)
            .child(Icon::new(icon).color(t::T2))
            .child(text(summary.clone()).text_color(t::T2))
            .child(Spinner::new().color(t::T2))
            .into_any_element(),
        ApprovalOutcome::Done(label) => done(label.clone()),
        ApprovalOutcome::Denied => note(IconName::CircleX, "Denied"),
        ApprovalOutcome::Failed(message) => failed(message.clone()),
        ApprovalOutcome::Rendering { job, file } => match state.render_job(*job).map(|j| j.state) {
            Some(RenderState::Queued) => card(("approval", id))
                .border_color(t::LN)
                .text_color(t::T2)
                .child(Icon::new(icon))
                .child(div().flex_none().child("Queued"))
                .child(
                    div()
                        .flex_1()
                        .child(Progress::new().indeterminate().h(px(3.))),
                )
                .into_any_element(),
            Some(RenderState::Running { fraction }) => card(("approval", id))
                .border_color(t::LN)
                .text_color(t::T2)
                .child(Icon::new(icon))
                .child(
                    h_flex().flex_none().gap(px(4.)).child("Rendering").child(
                        div()
                            .font_family(t::mono())
                            .child(format!("{:.0}%", (fraction * 100.0).clamp(0.0, 100.0))),
                    ),
                )
                .child(
                    div()
                        .flex_1()
                        .child(Progress::new().value(fraction).h(px(3.))),
                )
                .into_any_element(),
            Some(RenderState::Failed { message }) => failed(message),
            Some(RenderState::Canceled) => note(IconName::CircleX, "Canceled"),
            Some(RenderState::Done) | None => done(file.clone()),
        },
    }
}
