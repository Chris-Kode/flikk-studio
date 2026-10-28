//! Whisper (MIT, OpenAI) as ggml, run by whisper.cpp, with word times, in
//! three sizes: base by default, medium and large-v3 for fewer mistakes at
//! several times the memory and time.
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

pub const BASE: Model = Model {
    id: "whisper-base",
    name: "Whisper base",
    files: &[config::BASE],
};

pub const MEDIUM: Model = Model {
    id: "whisper-medium",
    name: "Whisper medium",
    files: &[config::MEDIUM],
};

pub const LARGE: Model = Model {
    id: "whisper-large-v3",
    name: "Whisper large-v3",
    files: &[config::LARGE],
};

/// Every size, smallest first.
pub const SIZES: [&Model; 3] = [&BASE, &MEDIUM, &LARGE];
