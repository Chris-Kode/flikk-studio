//! Bridge between the editor and `koo-agent`'s host.
//!
//! Owned by the agent work stream. Contract with `EditorState`:
//! `send` starts a turn, `pump` runs on the UI thread every frame-ish, serves
//! pending tool calls against the bus (as `Author::Agent`) and returns the
//! events produced since the last call.
//!
//! Everything else (backend subprocess, MCP bridge, tools, prompts) lives in
//! `koo_agent::AgentHost`; this type adapts it to the editor. [`approve`]
//! carries out the requests the user approves.

pub mod approve;

use koo_agent::{AgentHost, PendingApproval};
use koo_core::agent::{AgentEvent, AgentSettings, EditorContext};
use koo_core::bus::{AgentRole, CommandBus};
use koo_core::memory::{Recipe, SourceFact, StyleRule};
use koo_core::services::Services;

pub struct AgentLink {
    host: AgentHost,
}

impl AgentLink {
    pub fn new() -> Self {
        Self {
            host: AgentHost::new(),
        }
    }

    pub fn is_busy(&self) -> bool {
        self.host.is_busy()
    }

    pub fn send(&mut self, message: String, context: EditorContext, settings: &AgentSettings) {
        self.host.send(message, context, settings);
    }

    pub fn cancel(&mut self) {
        self.host.cancel();
    }

    pub fn pump(&mut self, bus: &mut CommandBus, services: &Services) -> Vec<AgentEvent> {
        self.host.pump(bus, services)
    }

    /// Editor or Builder role; applies from the next turn.
    pub fn set_role(&mut self, role: AgentRole) {
        self.host.set_role(role);
    }

    /// Recipes offered as `/name` slash commands.
    pub fn set_recipes(&mut self, recipes: Vec<Recipe>) {
        self.host.set_recipes(recipes);
    }

    /// Style rules and source facts from the store.
    pub fn set_memory(&mut self, rules: Vec<StyleRule>, facts: Vec<SourceFact>) {
        self.host.set_memory(rules, facts);
    }

    /// Memory and deliver requests waiting for the user's approval.
    pub fn pending_approvals(&self) -> &[PendingApproval] {
        self.host.pending_approvals()
    }

    /// Take a request off the list once the user approved (and the caller
    /// performed it) or denied it.
    pub fn resolve_approval(&mut self, id: u64) -> Option<PendingApproval> {
        self.host.resolve_approval(id)
    }

    /// What the user decided, told to the agent at the start of its next turn.
    pub fn report_outcome(&mut self, outcome: String) {
        self.host.report_outcome(outcome);
    }
}
