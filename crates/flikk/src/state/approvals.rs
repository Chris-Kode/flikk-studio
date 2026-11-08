//! Requests of the agent that wait on the user (memory, deliver,
//! generation): approving carries them out, and what came of each shows in
//! the chat until the user's next message.

use super::EditorState;
use crate::agent_link::approve;
use gpui_kit::*;

impl EditorState {
    /// Memory and deliver requests waiting for the user (PRD "Agent
    /// configuration": these categories need approval per call).
    pub fn pending_approvals(&self) -> Vec<flikk_agent::PendingApproval> {
        self.agent.pending_approvals()
    }

    pub fn deny_approval(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(pending) = self.agent.resolve_approval(id) else {
            return;
        };
        self.agent.report_failure(
            id,
            format!("denied by the user: {}", pending.request.summary),
        );
        self.settled.push((pending, ApprovalOutcome::Denied));
        cx.notify();
    }

    /// Carry out an approved request (store, render queue, export), take it
    /// off the list and tell the agent on its next turn.
    pub fn approve(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(pending) = self.agent.resolve_approval(id) else {
            return;
        };
        let summary = pending.request.summary.clone();
        let owner = self.bus.project().id;
        let mut settings = self.settings.clone();
        let started = approve::start(
            &pending.request,
            approve::Inputs {
                project: self.bus.project(),
                caption_styles: &self.presets().caption_styles,
                queue: &self.render_queue,
                services: &self.services,
                settings: &mut settings,
            },
        );
        if settings != self.settings {
            self.set_settings(settings, cx);
        } else {
            self.load_agent_memory();
        }
        let outcome = match started {
            Ok(approve::Started::Done(line)) => {
                self.agent
                    .report_outcome(id, format!("approved: {summary}"));
                if pending.request.tool == "save_fx_to_library" {
                    self.reload_library(cx);
                    if let Some(fx) = pending.request.args.get("fx").and_then(|v| v.as_str()) {
                        crate::ui::library::shared::link_to_library(self, fx, cx);
                    }
                }
                if pending.request.tool == "save_sound_to_library" {
                    self.reload_library(cx);
                }
                ApprovalOutcome::Done(line)
            }
            Ok(approve::Started::Render(job, path)) => {
                self.agent.report_outcome(
                    id,
                    format!("approved: {summary}; rendering to {}", path.display()),
                );
                ApprovalOutcome::Rendering {
                    job,
                    file: approve::file_name(&path),
                }
            }
            Ok(approve::Started::Background(work)) => {
                let what = summary.clone();
                let done = move |this: &mut Self, file: String, _: &mut Context<Self>| {
                    this.agent
                        .report_outcome(id, format!("approved: {what}; wrote {file}"));
                    ApprovalOutcome::Done(file)
                };
                self.settle_later(id, summary, work, done, cx)
            }
            Ok(approve::Started::Voice(work)) => {
                let what = summary.clone();
                let done = move |this: &mut Self, line: String, cx: &mut Context<Self>| {
                    this.agent
                        .report_outcome(id, format!("approved: {what}; {line}"));
                    this.reload_library(cx);
                    this.refresh_voices(cx);
                    ApprovalOutcome::Done("Voice kept in the library".into())
                };
                self.settle_later(id, summary, work, done, cx)
            }
            Ok(approve::Started::Respeak(work)) => {
                let what = summary.clone();
                let done =
                    move |this: &mut Self, made: approve::Respoken, cx: &mut Context<Self>| {
                        if this.bus.project().id != owner {
                            this.keep_approved(id, owner, made.item, &what)
                        } else {
                            this.place_respoken(id, made, cx)
                        }
                    };
                self.settle_later(id, summary, work, done, cx)
            }
            Ok(approve::Started::Generate(work)) => {
                let what = summary.clone();
                let done =
                    move |this: &mut Self, made: approve::Generated, cx: &mut Context<Self>| {
                        if this.bus.project().id != owner {
                            this.keep_approved(id, owner, made.item, &what)
                        } else {
                            this.place_generated(id, made, cx)
                        }
                    };
                self.settle_later(id, summary, work, done, cx)
            }
            Err(e) => self.approval_failed(id, &summary, e),
        };
        self.settled.push((pending, outcome));
        cx.notify();
    }

    /// Run approved request `id`'s `work` off the UI thread: its card shows
    /// it working until `done` turns what it made into the outcome (or it
    /// failed, which the agent is told).
    fn settle_later<T: Send + 'static>(
        &mut self,
        id: u64,
        summary: String,
        work: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) -> ApprovalOutcome + 'static,
        cx: &mut Context<Self>,
    ) -> ApprovalOutcome {
        let task = cx.background_executor().spawn(async move { work() });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                let outcome = match result {
                    Ok(made) => done(this, made, cx),
                    Err(e) => this.approval_failed(id, &summary, e),
                };
                if let Some(entry) = this.settled.iter_mut().find(|(p, _)| p.id == id) {
                    entry.1 = outcome;
                }
                cx.notify();
            });
        })
        .detach();
        ApprovalOutcome::Working
    }

    /// Approved request `id` (`summary`) failed with `e`: the agent is told.
    fn approval_failed(&mut self, id: u64, summary: &str, e: anyhow::Error) -> ApprovalOutcome {
        self.agent
            .report_failure(id, format!("approved but failed: {summary}: {e:#}"));
        ApprovalOutcome::Failed(format!("{e:#}"))
    }

    /// A render started from the chat, as the queue sees it now.
    pub fn render_job(&self, job: flikk_core::JobId) -> Option<flikk_deliver::RenderJob> {
        self.render_queue.jobs().into_iter().find(|j| j.id == job)
    }
}

/// What came of an approval request.
#[derive(Clone, Debug, PartialEq)]
pub enum ApprovalOutcome {
    Working,
    Done(String),
    Rendering {
        job: flikk_core::JobId,
        file: String,
    },
    Failed(String),
    Denied,
}

impl EditorState {
    /// The user's next message clears the requests they decided, except
    /// those still being carried out: their card stays to show how they
    /// end.
    pub(super) fn clear_settled(&mut self) {
        let queue = &self.render_queue;
        self.settled.retain(|(_, outcome)| match outcome {
            ApprovalOutcome::Working => true,
            ApprovalOutcome::Rendering { job, .. } => queue
                .jobs()
                .iter()
                .any(|j| j.id == *job && !j.state.is_finished()),
            _ => false,
        });
    }
}
