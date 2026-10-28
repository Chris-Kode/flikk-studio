//! Whisper (MIT, OpenAI) as ggml, run by whisper.cpp, with word times, in
//! four sizes: base by default; small and medium for fewer mistakes at
//! several times the memory and time; large-v3-turbo, large-v3 with its
//! decoder cut from 32 layers to 4, near large-v3's accuracy at about
//! medium's size.
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

pub const SMALL: Model = Model {
    id: "whisper-small",
    name: "Whisper small",
    files: &[config::SMALL],
};

pub const MEDIUM: Model = Model {
    id: "whisper-medium",
    name: "Whisper medium",
    files: &[config::MEDIUM],
};

pub const TURBO: Model = Model {
    id: "whisper-large-v3-turbo",
    name: "Whisper large-v3-turbo",
    files: &[config::TURBO],
};

/// Every size, smallest first.
pub const SIZES: [&Model; 4] = [&BASE, &SMALL, &MEDIUM, &TURBO];
