//! Domain model and command bus for koo-editor.
//!
//! Everything that changes a project goes through [`bus::CommandBus`]: the UI,
//! the agent (via MCP tools) and recipes all submit the same [`command::Command`]
//! values. A command is *planned* against the current project into a list of
//! invertible [`ops::Op`]s; the bus either applies them (one undo step) or, for
//! agent authors, parks them in a pending [`changeset::ChangeSet`].
//!
//! Other crates depend on this one and on nothing else in the workspace, so the
//! types here are the contract between them. See `docs/TECHNICAL_DESIGN.md`.

pub mod agent;
pub mod analysis;
pub mod audiofx;
pub mod bus;
pub mod captions;
pub mod changeset;
pub mod command;
pub mod compose;
pub mod cuts;
pub mod edit;
pub mod error;
pub mod fx;
pub mod grade;
pub mod heard;
pub mod history;
pub mod ids;
pub mod jobs;
pub mod library;
pub mod mask;
pub mod memory;
pub mod model;
pub mod ops;
pub mod paths;
pub mod query;
pub mod scopes;
pub mod services;
pub mod time;

pub use error::{CoreError, Result};
pub use ids::*;
pub use time::{Rate, Time, TimeRange};
