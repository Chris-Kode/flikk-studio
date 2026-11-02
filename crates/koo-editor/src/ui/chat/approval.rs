//! Approval cards (PRD: project/memory and deliver tools ask first), above
//! the composer: one line per request with Approve / Deny, then what came of
//! it in place of the buttons until the user's next message. A voice to be
//! added or read with plays from its card; approving a voice design opens
//! the Voices dialog, where the user hears the takes and keeps one.

use super::ChatPanel;
use crate::state::voices::{VoiceSample, approval_voice};
use crate::state::{ApprovalOutcome, EditorState};
use crate::ui::media_pool::listen_button;
use crate::ui::timeline::glyph::{Glyph, glyph};
use crate::ui::voices::{self, OnKept, Start};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_agent::PendingApproval;
use koo_agent::tools::ApprovalRequest;
use koo_core::command::ToolCategory;
use koo_deliver::RenderState;
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
            .map(|(p, o)| self.render_approval(p, o.as_ref(), cx))
            .collect()
    }

    fn render_approval(
        &self,
        approval: &PendingApproval,
        outcome: Option<&ApprovalOutcome>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = approval.id;
        let request = &approval.request;
        let summary: SharedString = request.summary.clone().into();
        let icon = match request.category {
            ToolCategory::Deliver => IconName::File,
            ToolCategory::Generate => IconName::Mic,
            _ => IconName::BookOpen,
        };
        let approve = Button::new(("approve", id as usize))
            .label("Approve")
            .xsmall()
            .primary();
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
        let trailing = match outcome {
            None => h_flex()
                .flex_none()
                .gap_1()
                .children(self.sample_button(request, id, cx))
                .child(
                    Button::new(("deny", id as usize))
                        .label("Deny")
                        .xsmall()
                        .ghost()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state.update(cx, |s, cx| s.deny_approval(id, cx))
                        })),
                )
                .child(approve)
                .into_any_element(),
            Some(outcome) => outcome_line(id, outcome, self.state.read(cx), cx),
        };
        let theme = cx.theme();
        h_flex()
            .mx_2()
            .mt_1()
            .min_h(px(36.))
            .pl_3()
            .pr_1p5()
            .py_1()
            .gap_2()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(theme.border)
            .bg(theme.secondary.opacity(0.4))
            .text_xs()
            .child(
                Icon::new(icon)
                    .size_3p5()
                    .flex_none()
                    .text_color(theme.muted_foreground),
            )
            .child(
                div()
                    .id(("approval", id as usize))
                    .flex_1()
                    .min_w_0()
                    .map(|el| match outcome {
                        // Readable in full before deciding.
                        None => el.line_clamp(2),
                        Some(_) => el.truncate().text_color(theme.muted_foreground),
                    })
                    .child(summary.clone())
                    .tooltip(move |window, cx| Tooltip::new(summary.clone()).build(window, cx)),
            )
            .child(trailing)
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
                .size(px(22.))
                .flex_none()
                .justify_center()
                .child(Spinner::new().xsmall())
                .into_any_element(),
            VoiceSample::Remote(voice) => Button::new(key)
                .ghost()
                .xsmall()
                .tooltip("Play")
                .child(glyph(Glyph::Play, px(13.), cx.theme().foreground))
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
        let Some((design, name)) = koo_agent::tools::voices::design_from_json(args) else {
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

fn outcome_line(id: u64, outcome: &ApprovalOutcome, state: &EditorState, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let (muted, green, red) = (theme.muted_foreground, theme.green, theme.red);
    let line = |icon: Option<IconName>, color: Hsla, text: String| {
        h_flex()
            .flex_none()
            .max_w(px(180.))
            .h(px(24.))
            .px_1p5()
            .gap_1p5()
            .text_xs()
            .text_color(color)
            .children(icon.map(|i| Icon::new(i).size_3p5().flex_none()))
            .child(div().min_w_0().truncate().child(text))
            .into_any_element()
    };
    let failed = |message: String| {
        let tip: SharedString = message.into();
        h_flex()
            .id(("approval-error", id as usize))
            .flex_none()
            .h(px(24.))
            .px_1p5()
            .gap_1p5()
            .text_xs()
            .text_color(red)
            .child(Icon::new(IconName::CircleAlert).size_3p5())
            .child("Failed")
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .into_any_element()
    };
    match outcome {
        ApprovalOutcome::Working => h_flex()
            .flex_none()
            .px_1p5()
            .child(Spinner::new().xsmall())
            .into_any_element(),
        ApprovalOutcome::Done(text) => line(Some(IconName::CircleCheck), green, text.clone()),
        ApprovalOutcome::Denied => line(None, muted, "Denied".into()),
        ApprovalOutcome::Failed(message) => failed(message.clone()),
        ApprovalOutcome::Rendering { job, file } => match state.render_job(*job).map(|j| j.state) {
            Some(RenderState::Queued) => line(None, muted, "Queued".into()),
            Some(RenderState::Running { fraction }) => line(
                None,
                muted,
                format!("Rendering {:.0}%", (fraction * 100.0).clamp(0.0, 100.0)),
            ),
            Some(RenderState::Failed { message }) => failed(message),
            Some(RenderState::Canceled) => line(None, muted, "Canceled".into()),
            Some(RenderState::Done) | None => {
                line(Some(IconName::CircleCheck), green, file.clone())
            }
        },
    }
}
