//! The agent's side of the editor: its models, its memory (rules and
//! facts), its settings, and what its turns bring in, drained
//! on the UI thread.

use super::EditorState;
use crate::persistence;
use flikk_core::ChangeSetId;
use flikk_core::agent::{AgentEvent, AgentSettings};
use gpui_kit::*;

impl EditorState {
    /// Ask the backend's CLI which models it offers, once per run. Until it
    /// answers the list from last time stands; a failed ask is tried again
    /// on the next call.
    pub fn load_agent_models(&mut self, cx: &mut Context<Self>) {
        let backend = self.settings.backend;
        if !self.models_asked.insert(backend) {
            return;
        }
        self.agent_models
            .entry(backend)
            .or_insert_with(|| flikk_agent::backend::cached_models(backend));
        let task = cx
            .background_executor()
            .spawn(async move { flikk_agent::backend::refresh_models(backend) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(models) => {
                        this.agent_models.insert(backend, models);
                    }
                    Err(_) => {
                        this.models_asked.remove(&backend);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Hand the agent the approved rules of the selected style profile and
    /// the facts about this project's sources.
    pub(super) fn load_agent_memory(&mut self) {
        self.agent.set_chat(self.chat.active());
        self.bus.set_chat(self.chat.active());
        use flikk_core::memory::{FactStatus, RuleStatus};
        use flikk_store::{FactFilter, RuleFilter};
        let rules = self
            .settings
            .style_profile
            .and_then(|profile| {
                let filter = RuleFilter {
                    status: Some(RuleStatus::Approved),
                    ..Default::default()
                };
                persistence::with_store(|s| s.rules(profile, &filter))
            })
            .unwrap_or_default();
        let fingerprints: Vec<_> = self
            .bus
            .project()
            .media
            .iter()
            .filter_map(|m| m.fingerprint.clone())
            .collect();
        let facts = fingerprints
            .into_iter()
            .flat_map(|fingerprint| {
                let filter = FactFilter {
                    fingerprint: Some(fingerprint),
                    ..Default::default()
                };
                persistence::with_store(|s| s.facts(&filter)).unwrap_or_default()
            })
            .filter(|f| f.status != FactStatus::Suspended)
            .collect();
        self.agent.set_memory(rules, facts);
    }

    pub fn cancel_agent(&mut self, cx: &mut Context<Self>) {
        self.agent.cancel();
        cx.notify();
    }

    pub fn set_settings(&mut self, settings: AgentSettings, cx: &mut Context<Self>) {
        self.bus.set_permissions(settings.permissions.clone());
        persistence::save_settings(&settings);
        let profile_changed = settings.style_profile != self.settings.style_profile;
        let backend_changed = settings.backend != self.settings.backend;
        self.settings = settings;
        self.load_agent_memory();
        if profile_changed {
            self.load_presets();
        }
        if backend_changed {
            self.load_agent_models(cx);
        }
        cx.notify();
    }

    pub(super) fn pump_agent(&mut self, cx: &mut Context<Self>) {
        let events = self.agent.pump(&mut self.bus, &self.services);
        // Proposals and closed sets; cheap while nothing changed.
        self.proposals.sync(&self.bus);
        // Sets that just went to review: what they bring in.
        self.check_change_sets(cx);
        // What its jobs brought in for a project closed meanwhile comes in
        // as its own.
        for (project, item) in self.agent.take_orphans() {
            let author = self.agent_author();
            self.keep_for(project, item, author);
        }
        // Requests the user allowed are carried out without asking; their
        // card still says what came of them.
        let allowed: Vec<u64> = self
            .agent
            .pending_approvals()
            .into_iter()
            .filter(|p| p.request.allowed)
            .map(|p| p.id)
            .collect();
        for id in allowed {
            self.approve(id, cx);
        }
        if events.is_empty() {
            return;
        }
        for event in events {
            match event {
                AgentEvent::TextDelta { text } => self.chat.append_text(text),
                AgentEvent::Question { text, choices } => self.chat.ask(text, choices),
                AgentEvent::ChangeSetReady { change_set } => self.post_change_set(change_set),
                AgentEvent::Activity { entry } => self.chat.push_tool(entry),
                AgentEvent::ToolStarted { entry, .. } => self.chat.push_tool(entry),
                AgentEvent::ToolFinished { call, entry } => self.chat.finish_tool(call, entry),
                AgentEvent::TurnDone => {
                    self.chat.end_turn();
                    self.save_chat()
                }
                AgentEvent::Progress { .. } => {}
                AgentEvent::Error { message } => {
                    self.chat.agent_failed(message.clone());
                    self.status = Some(message);
                }
            }
        }
        // A picture a job fetched from the web may have gone into the pool.
        self.autosave();
        cx.notify();
    }

    /// Close `change_set` for review. Its card sits above the composer
    /// rather than in the thread, so a set the chat grew over several turns
    /// shows once.
    pub(super) fn post_change_set(&mut self, change_set: ChangeSetId) {
        self.bus.close_change_set(change_set);
        self.chat.remove_change_set(change_set);
    }
}
