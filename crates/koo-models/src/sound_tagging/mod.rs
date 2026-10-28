//! Sound tagging: the sounds and music in an item's audio, and what the
//! music sounds like (genre, mood, vocals, lead instruments).

pub mod efficientat;

use crate::{Model, Windows};
use anyhow::Result;
use koo_core::analysis::{Levels, Music, Sound};
use koo_core::time::Time;

/// The models that can do it, the default first.
pub const MODELS: &[&Model] = &[&efficientat::MODEL];

/// What a tagger makes of one window; only it reads them.
pub type Scores = Vec<f32>;

/// Hears what sounds and music play, window by window.
pub trait Tagger: Send + Sync {
    /// How it hears the audio.
    fn windows(&self) -> Windows;

    /// Scores for each of `windows`, the last of which may be short.
    fn score(&self, windows: &[Vec<f32>]) -> Result<Vec<Scores>>;

    /// The sounds and music in `scores`, one per window, of audio `end`
    /// long. Its 10 ms `levels`, when known, place short sounds.
    fn read(
        &self,
        scores: &[Scores],
        end: Time,
        levels: Option<&Levels>,
    ) -> (Vec<Sound>, Vec<Music>);
}

/// The runner of `model`, one of [`MODELS`].
#[cfg(feature = "sounds")]
pub fn runner(model: &Model) -> Box<dyn Tagger> {
    match model.id {
        efficientat::ID => Box::<efficientat::EfficientAt>::default(),
        id => unreachable!("{id} is not a sound tagging model"),
    }
}
