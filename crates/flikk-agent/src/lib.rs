//! Agent host: runs the chosen backend CLI (Claude Code or Codex), serves
//! the editor tools over MCP, and streams `flikk_core::agent::AgentEvent`s.
//!
//! The pieces, in the order a turn uses them:
//!
//! * [`host::AgentHost`] owns a turn: it expands skills, builds the prompt
//!   ([`prompt`]), starts the backend subprocess ([`backend`]) and serves the
//!   tool calls that arrive over the [`bridge`] on the UI thread.
//! * The backend CLI launches `flikk-mcp` (this crate's binary, [`mcp`]), a
//!   stdio MCP server that forwards every call to the editor over
//!   `127.0.0.1` with a per-session token.
//! * [`tools`] is the catalog (one JSON schema per PRD tool) and the code that
//!   runs each tool against the `CommandBus` as `Author::Agent`.
//!
//! Beside the turns, [`reblock`] changes a blockout by asking: one backend
//! run without tools.

mod args;
pub mod backend;
pub mod bridge;
pub mod host;
pub mod mcp;
mod prompt;
pub mod reblock;
pub mod session;
pub mod tools;

pub use host::{AgentHost, PendingApproval};
