//! Transcription: the words said in the stretches voice activity found,
//! with their times, and the language they are in.

pub mod whisper;

use crate::Model;
use anyhow::Result;
use koo_core::analysis::Word;
use std::ops::Range;
use std::sync::Arc;

/// The models that can do it, the default first.
pub const MODELS: &[&Model] = &[&whisper::MODEL];

/// Turns speech into words.
pub trait Transcriber: Send + Sync {
    /// The words in the stretches `speech` (in samples) of
    /// [`SPEECH_RATE`](crate::SPEECH_RATE) mono `audio`. `language` is an
    /// ISO 639-1 hint; `None` detects it.
    fn transcribe(
        &self,
        audio: &[f32],
        speech: &[Range<usize>],
        language: Option<&str>,
        progress: Progress,
    ) -> Result<Transcribed>;

    /// The model's code for the two-letter language `tag`, if it knows it.
    fn language(&self, tag: &str) -> Option<&'static str>;
}

/// Takes 0..1, from whichever thread the model reports on.
pub type Progress = Arc<dyn Fn(f32) + Send + Sync>;

/// Recognized words and the detected (or given) language.
pub struct Transcribed {
    pub language: String,
    pub words: Vec<Word>,
}

/// The runner of `model`, one of [`MODELS`].
#[cfg(feature = "whisper")]
pub fn runner(model: &Model) -> Box<dyn Transcriber> {
    match model.id {
        whisper::ID => Box::<whisper::Whisper>::default(),
        id => unreachable!("{id} is not a transcription model"),
    }
}
