//! Qwen3-ForcedAligner-0.6B (Apache-2.0) hears a stretch of audio with the
//! words said in it and gives each word a start and an end, in 80 ms
//! steps. Whisper sometimes puts a word on the wrong sound and drifts by a
//! second in conversation; the aligner does not. It knows 11 languages
//! ([`config::LANGUAGES`]); elsewhere, and for words with no letters or
//! digits, the times analysis measured on the audio stay.

pub mod config;
#[cfg(feature = "aligner")]
mod runner;

use crate::Model;
#[cfg(feature = "aligner")]
pub use runner::Qwen3Aligner;

pub const ID: &str = "qwen3-forced-aligner-0.6b";

pub const MODEL: Model = Model {
    id: ID,
    name: "Qwen3 Aligner 0.6B",
    files: &[config::FILE, config::VOCAB, config::MERGES],
};
