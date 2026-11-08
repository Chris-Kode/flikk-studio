//! Fixing words by typing them, in the editor: the voice each speaker is
//! said in (a library voice linked to them), and the words said in it and
//! cut in. The user asking is their leave to spend the credits; the agent's
//! `replace_words` goes through the approval cards instead.

use super::cloud::Generating;
use super::{ApprovalOutcome, EditorState};
use crate::agent_link::approve;
use flikk_core::bus::Author;
use flikk_core::cloud::{CloudError, GenerationTask, Reading};
use flikk_core::library::{self, SavedVoice, VoiceSpeaker};
use flikk_core::respeak::{self, Fix};
use gpui_kit::*;

impl EditorState {
    /// The library voice `who` speaks with, when one is linked.
    pub fn speaker_voice(&self, who: &VoiceSpeaker) -> Option<&SavedVoice> {
        let provider = self.voice_provider()?;
        self.library.voice_of_speaker(&provider, who)
    }

    /// Remember that `who` speaks with `voice` (a provider's id), which the
    /// library keeps.
    pub fn link_speaker(&mut self, who: &VoiceSpeaker, voice: &str, cx: &mut Context<Self>) {
        let Some(provider) = self.voice_provider() else {
            return;
        };
        let Some(saved) = self.library.voice_of(&provider, voice) else {
            return;
        };
        match library::link_speaker(&self.library, saved.id, who) {
            Ok(()) => self.reload_library(cx),
            Err(e) => {
                self.say(e, cx);
            }
        }
    }

    /// Have the provider say `fix`'s words in `reading`'s voice, then add
    /// them to the pool and cut them in where the old words are, as the
    /// user's edit. Runs as a row in the media pool, which shows it fail.
    pub fn say_fix(&mut self, fix: Fix, reading: Reading, cx: &mut Context<Self>) {
        let Some(source) = self.bus.project().media(fix.media).cloned() else {
            return;
        };
        let id = self.next_generation;
        self.next_generation += 1;
        let task = GenerationTask::default();
        let owner = self.bus.project().id;
        self.generating.push(Generating {
            id,
            project: owner,
            request: respeak::request(&fix, &reading),
            task: task.clone(),
            failed: None,
        });
        let services = self.services.clone();
        let stopped = task.clone();
        let made = cx.background_executor().spawn({
            let fix = fix.clone();
            async move { respeak::make(&fix, &source, &reading, &services, &task) }
        });
        cx.spawn(async move |this, cx| {
            let result = made.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    // Its project was closed: the words go into its pool,
                    // to cut in by hand.
                    Ok(item) if this.bus.project().id != owner => {
                        this.generating.retain(|g| g.id != id);
                        this.keep_for(owner, item, Author::User);
                    }
                    // Stopped too late: what was made is paid for, so it
                    // goes into the pool, but isn't cut in.
                    Ok(item) if stopped.cancelled() => {
                        this.generating.retain(|g| g.id != id);
                        this.add_generated(item, cx);
                    }
                    Ok(item) => {
                        this.generating.retain(|g| g.id != id);
                        if let Some(media) = this.add_generated(item, cx) {
                            match fix.command(this.bus.project(), media) {
                                Ok(command) => {
                                    this.execute_batch(vec![command], "fix_words", cx);
                                }
                                Err(e) => this.status = Some(e),
                            }
                        }
                    }
                    // Stopped: it ends, without failing.
                    Err(_) if stopped.cancelled() => this.generating.retain(|g| g.id != id),
                    Err(e) => {
                        if let Some(g) = this.generating.iter_mut().find(|g| g.id == id) {
                            g.failed = Some(CloudError::Failed(e));
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Add the words an approved fix had said to the pool and propose them
    /// in place of the old ones in the chat's change set; tell the agent.
    pub(super) fn place_respoken(
        &mut self,
        id: u64,
        made: approve::Respoken,
        cx: &mut Context<Self>,
    ) -> ApprovalOutcome {
        let approve::Respoken { item, fix, summary } = made;
        let name = approve::file_name(&item.path);
        let author = self.agent_author();
        let Some(media) = self.add_generated_as(item, author, cx) else {
            let e = self.status.clone().unwrap_or_default();
            self.agent.report_failure(
                id,
                format!("approved: {summary}; made {name}, but could not add it to the pool: {e}"),
            );
            return ApprovalOutcome::Failed(e);
        };
        let mut outcome = format!("approved: {summary}; made media {media} ({name})");
        match self.with_agent_set(cx, |bus, author| {
            flikk_agent::tools::respeak::propose(bus, author, media, &fix, &summary)
        }) {
            Ok(text) => outcome.push_str(&format!("; {text}")),
            Err(e) => outcome.push_str(&format!("; could not cut it in: {e}")),
        }
        self.agent.report_outcome(id, outcome);
        ApprovalOutcome::Done(name)
    }
}
