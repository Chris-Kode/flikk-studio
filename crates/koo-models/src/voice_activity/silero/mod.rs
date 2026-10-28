//! Silero VAD v6.2 (MIT), run by whisper.cpp. Unlike the energy VAD
//! analysis falls back on without it, it does not take music for speech.

pub mod config;
#[cfg(feature = "whisper")]
mod runner;

use crate::Model;
#[cfg(feature = "whisper")]
pub use runner::Silero;

pub const ID: &str = "silero-vad-6.2";

pub const MODEL: Model = Model {
    id: ID,
    name: "Silero VAD 6.2",
    files: &[config::FILE],
};
