//! Approval cards (PRD: project/memory and deliver tools ask first): one
//! line per request under the agent message that made it, Approve / Deny,
//! then the outcome in place of the buttons.

use super::ChatPanel;
use crate::state::{ApprovalCard, ApprovalOutcome, EditorState};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::command::ToolCategory;
use koo_deliver::RenderState;
use std::time::Duration;

const RENDER_POLL: Duration = Duration::from_millis(250);

impl ChatPanel {
    pub(super) fn render_approvals(&self, ix: usize, cx: &mut Context<Self>) -> Option<Div> {
        let cards: Vec<ApprovalCard> = self
            .state
            .read(cx)
            .approvals
            .iter()
            .filter(|c| c.message == ix)
            .cloned()
            .collect();
        (!cards.is_empty()).then(|| {
            v_flex()
                .w_full()
                .gap_1()
                .children(cards.iter().map(|c| self.render_approval(c, cx)))
        })
    }

    fn render_approval(&self, card: &ApprovalCard, cx: &mut Context<Self>) -> AnyElement {
        let id = card.approval.id;
        let request = &card.approval.request;
        let summary: SharedString = request.summary.clone().into();
        let icon = match request.category {
            ToolCategory::Deliver => IconName::File,
            _ => IconName::BookOpen,
        };
        let trailing = match &card.outcome {
            None => h_flex()
                .flex_none()
                .gap_1()
                .child(
                    Button::new(("deny", id as usize))
                        .label("Deny")
                        .xsmall()
                        .ghost()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state.update(cx, |s, cx| s.deny(id, cx))
                        })),
                )
                .child(
                    Button::new(("approve", id as usize))
                        .label("Approve")
                        .xsmall()
                        .primary()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state.update(cx, |s, cx| s.approve(id, cx))
                        })),
                )
                .into_any_element(),
            Some(outcome) => outcome_line(id, outcome, self.state.read(cx), cx),
        };
        let theme = cx.theme();
        h_flex()
            .w_full()
            .min_h(px(36.))
            .pl_3()
            .pr_1p5()
            .py_1()
            .gap_2()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(theme.border)
            .bg(theme.secondary.opacity(0.4))
            .text_sm()
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
                    .map(|el| match card.outcome {
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
    state.approvals.iter().any(|c| match &c.outcome {
        Some(ApprovalOutcome::Rendering { job, .. }) => state
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
