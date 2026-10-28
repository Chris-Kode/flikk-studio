//! Whisper (MIT, OpenAI) as ggml, run by whisper.cpp, with word times.
//!
//! Whisper given music or a long silence makes words up there, often a
//! verbatim repeat of what was said before, and its timestamps come loose
//! around them. So, as faster-whisper's VAD filter does, voice activity
//! finds the speech first and Whisper hears only that, each stretch on its
//! own: one that starts with music taken for speech would throw the times
//! of the speech after it.

pub mod config;
#[cfg(feature = "whisper")]
mod runner;

use crate::Model;
#[cfg(feature = "whisper")]
pub use runner::Whisper;

pub const ID: &str = "whisper-base";

pub const MODEL: Model = Model {
    id: ID,
    name: "Whisper base",
    files: &[config::FILE],
};
