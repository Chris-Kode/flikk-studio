//! Agent host: runs the chosen backend CLI (Claude Code or Codex), serves
//! the editor tools over MCP, and streams `koo_core::agent::AgentEvent`s.
//!
//! The pieces, in the order a turn uses them:
//!
//! * [`host::AgentHost`] owns a turn: it expands recipes, builds the prompt
//!   ([`prompt`]), starts the backend subprocess ([`backend`]) and serves the
//!   tool calls that arrive over the [`bridge`] on the UI thread.
//! * The backend CLI launches `koo-mcp` (this crate's binary, [`mcp`]), a
//!   stdio MCP server that forwards every call to the editor over
//!   `127.0.0.1` with a per-session token.
//! * [`tools`] is the catalog (one JSON schema per PRD tool) and the code that
//!   runs each tool against the `CommandBus` as `Author::Agent`.

pub mod args;
pub mod backend;
pub mod bridge;
pub mod host;
pub mod mcp;
pub mod prompt;
pub mod recipes;
pub mod session;
pub mod tools;

pub use host::{AgentHost, PendingApproval};
