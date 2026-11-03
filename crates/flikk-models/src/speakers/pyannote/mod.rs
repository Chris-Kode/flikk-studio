//! Who speaks when, as pyannote's speaker-diarization-3.1 pipeline finds
//! it, on ONNX Runtime:
//!
//! 1. pyannote segmentation-3.0 (MIT) hears 10 s windows a
//!    [`config::STEP`] apart and says, every 17 ms, which of up to three
//!    voices of that window speak, overlaps included;
//! 2. WeSpeaker ResNet34-LM (CC-BY-4.0, trained on VoxCeleb) turns each
//!    window's voice, heard where it speaks alone, into a 256-number
//!    voiceprint;
//! 3. the voiceprints are clustered (centroid linkage, cut at pyannote's
//!    tuned threshold, small clusters folded into the nearest large one),
//!    so each cluster is one voice across the whole item;
//! 4. each window's voices are renamed after their clusters, the windows
//!    are averaged, and every 17 ms keeps as many voices as the windows
//!    heard speaking at once there.
//!
//! A word goes to the voice that speaks through most of it.

pub mod config;
#[cfg(feature = "diarizer")]
mod fbank;
#[cfg(feature = "diarizer")]
mod runner;

use crate::Model;
#[cfg(feature = "diarizer")]
pub use runner::Pyannote;

pub const ID: &str = "pyannote-3.1";

pub const MODEL: Model = Model {
    id: ID,
    name: "pyannote + WeSpeaker",
    files: &[config::SEGMENTATION, config::EMBEDDING],
};
