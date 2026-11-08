//! Operating-system plumbing every Flikk crate shares, so each rule lives
//! in one place (AGENTS.md, "State and async work"):
//!
//! - [`fs`]: files written whole or not at all, and names claimed without
//!   ever replacing a file this operation did not create;
//! - [`process`]: child processes with no console window that are killed
//!   and reaped when their owner lets go of them, and where to find FFmpeg;
//! - [`sync`]: locks that survive a panic of their holder, and the text of
//!   a caught panic;
//! - [`backoff`]: failures remembered for a while and then tried again,
//!   never cached for the session.
//!
//! Only `std`: every crate of the workspace may depend on it.

pub mod backoff;
pub mod fs;
pub mod process;
pub mod sync;
