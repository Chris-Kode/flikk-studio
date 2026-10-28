//! Whisper (MIT, OpenAI) as ggml, run by whisper.cpp, with word times, in
//! two sizes, both with 8-bit weights: small by default, and large
//! (large-v3-turbo: large-v3 with its decoder cut from 32 layers to 4,
//! near large-v3's accuracy) for fewer mistakes at about three times the
//! size.
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

pub const SMALL: Model = Model {
    id: "whisper-small-q8",
    name: "Whisper small",
    files: &[config::SMALL],
};

pub const LARGE: Model = Model {
    id: "whisper-large-v3-turbo-q8",
    name: "Whisper large",
    files: &[config::LARGE],
};

/// Every size, smallest first.
pub const SIZES: [&Model; 2] = [&SMALL, &LARGE];
