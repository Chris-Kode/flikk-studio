//! A state as one of the open projects: what it shares with the others,
//! what it reads again when one of them changes it, and what it does on
//! coming into the editor, leaving it and closing.

use super::EditorState;
use crate::persistence;
use flikk_core::agent::{AgentModel, Backend};
use flikk_core::model::Fingerprint;
use flikk_core::services::Services;
use gpui_kit::*;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// What the states of the open projects share: the engines, the providers,
/// the model downloads, the render queue and the models each agent CLI
/// offers. Clones are handles to the same things.
pub struct Shared {
    pub(super) services: Services,
    pub(super) cloud: Arc<flikk_cloud::Cloud>,
    pub(super) models: flikk_models::Downloads,
    pub(super) render_queue: flikk_deliver::RenderQueue,
    pub(super) agent_models: HashMap<Backend, Vec<AgentModel>>,
}

/// Something every open project uses was changed through this state and
/// saved: the others read it again ([`EditorState::read_again`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SharedChange {
    /// The agent settings.
    Settings,
    /// The effects, presets, sounds and voices of the library.
    Library,
    /// The selected style profile's presets.
    Presets,
    /// A provider's key, or which provider does what.
    Providers,
}

impl EventEmitter<SharedChange> for EditorState {}

impl EditorState {
    /// Handles to what this state shares, for a project opened beside it.
    pub fn shared(&self) -> Shared {
        let mut services = self.services.clone();
        // Lettering follows each project's own caption styles.
        services.lettering = None;
        Shared {
            services,
            cloud: self.cloud.clone(),
            models: self.models.clone(),
            render_queue: self.render_queue.clone(),
            agent_models: self.agent_models.clone(),
        }
    }

    /// Another open project's state saved `change`: read it again, as
    /// saved. Nothing is saved or announced from here.
    pub fn read_again(&mut self, change: SharedChange, cx: &mut Context<Self>) {
        match change {
            SharedChange::Settings => {
                let Some(settings) = persistence::with_store(|s| s.user_settings()) else {
                    return;
                };
                if settings == self.settings {
                    return;
                }
                self.bus.set_permissions(settings.permissions.clone());
                let profile_changed = settings.style_profile != self.settings.style_profile;
                self.settings = settings;
                self.load_agent_memory();
                if profile_changed {
                    self.load_presets();
                }
            }
            SharedChange::Library => {
                self.library = flikk_core::library::Library::load();
            }
            SharedChange::Presets => self.load_presets(),
            SharedChange::Providers => {
                self.configure_analysis();
                self.load_keys(cx);
            }
        }
        cx.notify();
    }

    /// Whether the project shows with the open projects on the home page.
    pub fn is_listed(&self) -> bool {
        self.listed
    }

    /// Whether this is the project in the editor.
    pub fn in_editor(&self) -> bool {
        self.in_editor
    }

    /// The project took the editor: external MCP clients' calls that name
    /// no project are for it now.
    pub fn enter_editor(&mut self, cx: &mut Context<Self>) {
        self.in_editor = true;
        cx.notify();
    }

    /// The user went into the project: it shows on the home page from now
    /// on, also the one the app started on.
    pub fn list(&mut self, cx: &mut Context<Self>) {
        if !self.listed {
            self.listed = true;
            cx.notify();
        }
    }

    /// Hide the project from the home page: the "Untitled" that stands in
    /// when the last open project closes.
    pub fn unlist(&mut self) {
        self.listed = false;
    }

    /// Another project takes the editor: what only the editor does for a
    /// project stops. Its agent goes on working.
    pub fn leave_editor(&mut self, cx: &mut Context<Self>) {
        self.in_editor = false;
        self.set_playing(false, cx);
        // The pool item in the viewer plays on its own clock.
        self.source = None;
        self.clear_live();
        cx.notify();
    }

    /// The project closes: stop its agent, with what its turn got to saved
    /// in its chat and its proposals kept for review, and the background
    /// work on media no other open project has (`elsewhere`: the
    /// fingerprints of theirs).
    pub fn shut_down(&mut self, elsewhere: &HashSet<Fingerprint>, cx: &mut Context<Self>) {
        self.leave_editor(cx);
        self.agent.cancel();
        // The stopped turn ends on this pump.
        self.pump_agent(cx);
        self.save_chat();
        self.proposals.sync(&self.bus);
        let sounds = self.library.sounds.iter().map(|s| &s.fingerprint);
        let kept: HashSet<&Fingerprint> = sounds.flatten().chain(elsewhere).collect();
        let media = self.bus.project().media.iter();
        let left = media.filter(|m| m.fingerprint.as_ref().is_some_and(|fp| !kept.contains(fp)));
        for item in left {
            if let Some(analysis) = &self.services.analysis {
                analysis.cancel(item);
            }
            if let Some(stabilizer) = &self.services.stabilizer {
                stabilizer.cancel(item);
            }
        }
    }

    /// The fingerprints of the project's media, for a closing project to
    /// leave their background work running.
    pub fn fingerprints(&self) -> impl Iterator<Item = Fingerprint> + '_ {
        let media = self.bus.project().media.iter();
        media.filter_map(|m| m.fingerprint.clone())
    }
}
