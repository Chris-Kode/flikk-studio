//! The tagger's file, and how it is run and read.

use crate::{ModelFile, SOUND_RATE};
use koo_core::time::{FLICKS_PER_SECOND, Time};

pub const FILE: ModelFile = ModelFile {
    name: "efficientat-mn10-as.onnx",
    url: "https://huggingface.co/Chris-Kode/efficientat-mn10-as-onnx/resolve/81f0e383972c52b354de924778fec25b2a4a29d3/efficientat-mn10-as.onnx",
    env: Some("KOOEDITOR_TAGGER_MODEL"),
    what: "sound tagger",
    size: 20_200_590,
};

/// Samples the tagger hears at once, and between the starts of windows.
pub const WINDOW: usize = 2 * SOUND_RATE as usize;
pub const STEP: usize = SOUND_RATE as usize;
pub const WINDOW_TIME: Time = Time(FLICKS_PER_SECOND * 2);
pub const STEP_TIME: Time = Time(FLICKS_PER_SECOND);
/// What the model is given: a window and then silence.
pub const INPUT: usize = 6 * SOUND_RATE as usize;
/// Windows run at once.
pub const BATCH: usize = 32;
/// AudioSet classes the model scores.
pub const CLASSES: usize = 527;

/// A sound starts at this probability and lasts while it stays above
/// `SOUND_OFF`. Speech alone keeps every other class below 0.05.
pub const SOUND_ON: f32 = 0.25;
pub const SOUND_OFF: f32 = 0.12;
/// Music needs more, and longer: silence alone scores 0.25 as music, and a
/// chord struck once is not music.
pub const MUSIC_ON: f32 = 0.5;
pub const MUSIC_OFF: f32 = 0.3;
pub const MUSIC_MIN: Time = Time(FLICKS_PER_SECOND * 3);
/// Runs of one label closer than this are one sound (in windows).
pub const SOUND_GAP: usize = 1;
pub const MUSIC_GAP: usize = 2;
