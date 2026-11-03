//! Sound search: vectors for finding a sound by description, one for each
//! window of an item's audio and one for a description; the closer two
//! are, the better the description fits.

pub mod clap;

use crate::{Model, Windows};
use anyhow::Result;

/// The models that can do it, the default first.
pub const MODELS: &[&Model] = &[&clap::MODEL];

/// Puts sounds and descriptions of them in one space. Every vector holds
/// [`SoundVectors::DIMS`](flikk_core::analysis::SoundVectors::DIMS) numbers.
pub trait SoundSearch: Send + Sync {
    /// How it hears the audio, a vector per window.
    fn windows(&self) -> Windows;

    /// The vector of each of `windows`, the last of which may be short;
    /// silent ones get zeros.
    fn embed(&self, windows: &[&[f32]]) -> Result<Vec<Vec<f32>>>;

    /// The vector of a description.
    fn describe(&self, text: &str) -> Result<Vec<f32>>;
}

/// The runner of `model`, one of [`MODELS`].
#[cfg(feature = "sounds")]
pub fn runner(model: &Model) -> Box<dyn SoundSearch> {
    match model.id {
        clap::ID => Box::<clap::Clap>::default(),
        id => unreachable!("{id} is not a sound search model"),
    }
}
