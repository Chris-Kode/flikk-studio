//! The timeline check (`flikk_core::check`) for the editor: change sets
//! waiting for review are checked off the UI thread and keep what they
//! bring in, for their card and Review list; Deliver checks its range with
//! [`EditorState::check_sources`].

use super::EditorState;
use flikk_core::check::{self, Sources};
use gpui_kit::*;

impl EditorState {
    /// What a check reads besides the project: the engines, and the media
    /// found offline.
    pub fn check_sources(&self) -> Sources {
        Sources {
            services: self.services.clone(),
            offline: self.offline.ids().clone(),
        }
    }

    /// Check every change set that went to review since the last call, each
    /// on its own background task; what it brings in lands on the set. A
    /// result for a set that changed or went meanwhile, or for a project
    /// no longer open, is dropped by the bus.
    pub(super) fn check_change_sets(&mut self, cx: &mut Context<Self>) {
        while let Some(job) = self.bus.take_check() {
            let sources = self.check_sources();
            cx.spawn(async move |this, cx| {
                let (set, round, project) = (job.set, job.round, job.after.id);
                let found = cx
                    .background_executor()
                    .spawn(async move {
                        // A check that panics brings in nothing rather than
                        // leaving the set checking for good.
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            let before = check::check(&job.before, None, &sources);
                            let after = check::check(&job.after, None, &sources);
                            check::introduced(&before.findings, &after.findings)
                        }))
                        .unwrap_or_default()
                    })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    if this.bus.project().id == project {
                        this.bus.keep_findings(set, round, found);
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }
}
