//! Word timing: where each word of a transcript starts and ends, heard
//! against the audio. A transcriber only knows roughly where a word is.

pub mod qwen3_aligner;

use crate::Model;
use anyhow::Result;
use koo_core::analysis::Word;

/// The models that can do it, the default first.
pub const MODELS: &[&Model] = &[&qwen3_aligner::MODEL];

/// Puts words where they are said.
pub trait Aligner: Send + Sync {
    /// Whether it knows `language` (an ISO 639-1 code, as transcription
    /// gives it). Words in another keep the times they have.
    fn supports(&self, language: &str) -> bool;

    /// Move `words` onto where the model hears them in
    /// [`SPEECH_RATE`](crate::SPEECH_RATE) mono `audio`. Their times only
    /// need to be roughly right; they decide which stretch of audio each
    /// word is looked for in.
    fn align(&self, audio: &[f32], words: &mut [Word], progress: &dyn Fn(f32)) -> Result<()>;
}

/// The runner of `model`, one of [`MODELS`].
#[cfg(feature = "aligner")]
pub fn runner(model: &Model) -> Box<dyn Aligner> {
    match model.id {
        qwen3_aligner::ID => Box::<qwen3_aligner::Qwen3Aligner>::default(),
        id => unreachable!("{id} is not a word timing model"),
    }
}
