//! Speakers: who says each word of a transcript, as voices told apart in
//! the audio.

pub mod pyannote;

use crate::Model;
use anyhow::Result;
use koo_core::analysis::Word;

/// The models that can do it, the default first.
pub const MODELS: &[&Model] = &[&pyannote::MODEL];

/// Number of distinct voices and, for each word, the index of its voice
/// with a 0..1 confidence (`None` when it cannot tell).
pub type Labels = (usize, Vec<Option<(usize, f32)>>);

/// Groups words by voice.
pub trait Diarizer: Send + Sync {
    /// The voice of each of `words`. `audio` is
    /// [`SPEECH_RATE`](crate::SPEECH_RATE) mono for the same source times
    /// as the words; `progress` takes 0..1.
    fn diarize(&self, audio: &[f32], words: &[Word], progress: &dyn Fn(f32)) -> Result<Labels>;
}

/// The runner of `model`, one of [`MODELS`].
#[cfg(feature = "diarizer")]
pub fn runner(model: &Model) -> Box<dyn Diarizer> {
    match model.id {
        pyannote::ID => Box::<pyannote::Pyannote>::default(),
        id => unreachable!("{id} is not a speakers model"),
    }
}
