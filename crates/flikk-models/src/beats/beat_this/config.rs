//! The beat tracker's file, and how it is run.

use crate::{ModelFile, SOUND_RATE};
use flikk_core::time::{FLICKS_PER_SECOND, Time};

pub const FILE: ModelFile = ModelFile {
    name: "beat-this-final0.onnx",
    url: "https://huggingface.co/Chris-Kode/beat-this-final0-onnx/resolve/49876475cff9b3abe3eec5303fa539cff06843fd/beat-this-final0.onnx",
    env: Some("FLIKK_BEATS_MODEL"),
    what: "beat tracker",
    size: 84_973_959,
};

/// Samples per 20 ms frame.
pub const FRAME: usize = SOUND_RATE as usize / 50;
pub const FRAME_TIME: Time = Time(FLICKS_PER_SECOND / 50);
/// Frames the model hears at once, as it was trained.
pub const CHUNK: usize = 1500;
/// Frames at each end of a chunk whose predictions are unreliable.
pub const BORDER: usize = 6;
pub const HOP: usize = CHUNK - 2 * BORDER;
