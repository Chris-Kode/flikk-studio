//! The agent's side of the editor: its models, its memory (rules and
//! facts), its settings, and what its turns bring in, drained
//! on the UI thread.

use super::EditorState;
use super::chat::ChatLog;
use crate::agent_link::AgentLink;
use crate::persistence;
use flikk_core::ChangeSetId;
use flikk_core::agent::{AgentEvent, AgentSettings};
use flikk_core::bus::CommandBus;
use flikk_core::changeset::ChangeSetState;
use gpui_kit::*;

/// Hand `agent` what it works from on the project of `bus`: the open chat
/// of `chat`, the approved rules of the style profile selected in
/// `settings`, and the facts about the project's sources.
pub(super) fn hand_memory(
    settings: &AgentSettings,
    bus: &mut CommandBus,
    chat: &ChatLog,
    agent: &mut AgentLink,
) {
    agent.set_chat(chat.active());
    bus.set_chat(chat.active());
    use flikk_core::memory::{FactStatus, RuleStatus};
    use flikk_store::{FactFilter, RuleFilter};
    let rules = settings
        .style_profile
        .and_then(|profile| {
            let filter = RuleFilter {
                status: Some(RuleStatus::Approved),
                ..Default::default()
            };
            persistence::with_store(|s| s.rules(profile, &filter))
        })
        .unwrap_or_default();
    let fingerprints: Vec<_> = (bus.project().media.iter())
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
    agent.set_memory(rules, facts);
}

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

    /// Hand every open project's agent the approved rules of the selected
    /// style profile and the facts about its own project's sources.
    pub(super) fn load_agent_memory(&mut self) {
        hand_memory(&self.settings, &mut self.bus, &self.chat, &mut self.agent);
        let settings = self.settings.clone();
        for parked in self.parked_mut() {
            parked.hand_memory(&settings);
        }
    }

    pub fn cancel_agent(&mut self, cx: &mut Context<Self>) {
        self.agent.cancel();
        cx.notify();
    }

    pub fn set_settings(&mut self, settings: AgentSettings, cx: &mut Context<Self>) {
        self.bus.set_permissions(settings.permissions.clone());
        for parked in self.parked_mut() {
            parked
                .bus_mut()
                .set_permissions(settings.permissions.clone());
        }
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
        // What its jobs brought in for a project closed meanwhile comes in
        // as its own.
        for (project, item) in self.agent.take_orphans() {
            let author = self.agent_author();
            self.keep_for(project, item, author);
        }
        // Requests the user allowed are carried out without asking, and so
        // is every request of an unattended turn; their card still says
        // what came of them.
        let unattended = self.unattended;
        let allowed: Vec<u64> = self
            .agent
            .pending_approvals()
            .into_iter()
            .filter(|p| p.request.allowed || unattended)
            .map(|p| p.id)
            .collect();
        if unattended && !allowed.is_empty() {
            // What is approved works on the project as the agent left it:
            // a render must show the edits of the turn that asked for it.
            self.accept_agent_edits(cx);
        }
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
                AgentEvent::ChangeSetReady { change_set } => {
                    self.post_change_set(change_set);
                    if unattended {
                        self.accept_agent_edits(cx);
                    }
                }
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
        // A picture or video a job fetched from the web may have gone into
        // the pool: save it, and analyze a video as any import is.
        self.autosave();
        self.analyze_new(cx);
        if unattended && !self.provider_asks.is_empty() {
            self.analyze_asked_with_provider(cx);
        }
        cx.notify();
    }

    /// Apply the edits the agent proposed so far, each change set as one
    /// undo step: an unattended turn's edits wait for no review. Items
    /// that conflict stay for review.
    pub(super) fn accept_agent_edits(&mut self, cx: &mut Context<Self>) {
        let waiting: Vec<ChangeSetId> = (self.bus.change_sets().iter())
            .filter(|set| set.author.is_agent() && set.state != ChangeSetState::Resolved)
            .filter(|set| set.pending_items().next().is_some())
            .map(|set| set.id)
            .collect();
        for set in waiting {
            self.bus.close_change_set(set);
            self.accept(set, None, cx);
        }
    }

    /// Close `change_set` for review. Its card sits above the composer
    /// rather than in the thread, so a set the chat grew over several turns
    /// shows once.
    pub(super) fn post_change_set(&mut self, change_set: ChangeSetId) {
        self.bus.close_change_set(change_set);
        self.chat.remove_change_set(change_set);
    }
}
