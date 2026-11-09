//! Bridge between the editor and `flikk-agent`'s host.
//!
//! Owned by the agent work stream. Contract with `EditorState`:
//! `send` starts a turn, `pump` runs on the UI thread every frame-ish, serves
//! pending tool calls against the bus (as `Author::Agent`) and returns the
//! events produced since the last call.
//!
//! Everything else (backend subprocess, MCP bridge, tools, prompts) lives in
//! `flikk_agent::AgentHost`; this type adapts it to the editor. [`approve`]
//! carries out the requests the user approves.
//!
//! The bridge starts with the editor and is published in
//! `paths::home()/bridge.json`, so external MCP clients can drive it:
//! `claude mcp add flikk -- flikk mcp` (or the `flikk-mcp` binary) finds
//! the running editor through that file when `FLIKK_BRIDGE` is unset.

pub mod approve;

use flikk_agent::host::TurnStatus;
use flikk_agent::{AgentHost, PendingApproval};
use flikk_core::agent::{AgentEvent, AgentSettings, EditorContext};
use flikk_core::bus::CommandBus;
use flikk_core::memory::{SourceFact, StyleRule};
use flikk_core::services::Services;

pub struct AgentLink {
    host: AgentHost,
}

impl AgentLink {
    pub fn new() -> Self {
        let mut host = AgentHost::new();
        if let Err(e) = host.start_bridge() {
            eprintln!("flikk: external agents can't connect: {e}");
        }
        Self { host }
    }

    /// Whether a message is queued or its turn under way.
    pub fn status(&self) -> TurnStatus {
        self.host.status()
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

    /// Chat session whose backend conversation the next turn continues.
    pub fn set_chat(&mut self, chat: &str) {
        self.host.set_chat(chat);
    }

    /// Style rules and source facts from the store.
    pub fn set_memory(&mut self, rules: Vec<StyleRule>, facts: Vec<SourceFact>) {
        self.host.set_memory(rules, facts);
    }

    /// Requests waiting for the user's approval in the open chat.
    pub fn pending_approvals(&self) -> Vec<PendingApproval> {
        self.host.pending_approvals()
    }

    /// The user opened another project: withdraw the requests `project`
    /// left waiting.
    pub fn close_project(&mut self, project: flikk_core::ProjectId) {
        self.host.close_project(project);
    }

    /// What the agent made for projects closed before it finished.
    pub fn take_orphans(&mut self) -> Vec<(flikk_core::ProjectId, flikk_core::model::MediaItem)> {
        self.host.take_orphans()
    }

    /// Take a request off the list once the user approved (and the caller
    /// performed it) or denied it.
    pub fn resolve_approval(&mut self, id: u64) -> Option<PendingApproval> {
        self.host.resolve_approval(id)
    }

    /// What came of request `id`: the result of the call still waiting for
    /// it, else told to the agent at the start of its next turn.
    pub fn report_outcome(&mut self, id: u64, outcome: String) {
        self.host.report_outcome(id, outcome, false);
    }

    /// Request `id` was denied or failed; reported like
    /// [`report_outcome`](Self::report_outcome), as an error.
    pub fn report_failure(&mut self, id: u64, outcome: String) {
        self.host.report_outcome(id, outcome, true);
    }
}

impl Drop for AgentLink {
    fn drop(&mut self) {
        self.host.stop_bridge();
    }
}

/// Withdraw `bridge.json` when the app quits (the link may not be dropped).
pub fn unpublish_bridge() {
    flikk_agent::bridge::Discovery::remove_if_ours();
}
