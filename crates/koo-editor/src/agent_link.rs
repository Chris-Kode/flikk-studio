//! Bridge between the editor and `koo-agent`'s host.
//!
//! Owned by the agent work stream. Contract with `EditorState`:
//! `send` starts a turn, `pump` runs on the UI thread every frame-ish, serves
//! pending tool calls against the bus (as `Author::Agent`) and returns the
//! events produced since the last call.

use koo_core::agent::{AgentEvent, AgentSettings, EditorContext};
use koo_core::bus::CommandBus;
use koo_core::services::Services;

pub struct AgentLink {
    pending: Vec<AgentEvent>,
}

impl AgentLink {
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
        }
    }

    pub fn is_busy(&self) -> bool {
        false
    }

    pub fn send(&mut self, _message: String, _context: EditorContext, _settings: &AgentSettings) {
        self.pending.push(AgentEvent::Error {
            message: "The agent is not connected yet".into(),
        });
    }

    pub fn cancel(&mut self) {}

    pub fn pump(&mut self, _bus: &mut CommandBus, _services: &Services) -> Vec<AgentEvent> {
        std::mem::take(&mut self.pending)
    }
}
