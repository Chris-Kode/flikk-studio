//! Beats: the beats and downbeats of a stretch of music.

pub mod beat_this;

use crate::Model;
use anyhow::Result;
use flikk_core::time::Time;

/// The models that can do it, the default first.
pub const MODELS: &[&Model] = &[&beat_this::MODEL];

/// Hands a stretch's samples, as they decode, to the sink it is given.
pub type Stream<'a> = &'a mut dyn FnMut(&mut dyn FnMut(&[f32]) -> Result<()>) -> Result<()>;

/// Finds the beat of music.
pub trait BeatTracker: Send + Sync {
    /// The beats and downbeats of the [`SOUND_RATE`](crate::SOUND_RATE)
    /// mono audio `stream` hands over, from its start.
    fn track(&self, stream: Stream) -> Result<(Vec<Time>, Vec<Time>)>;
}

/// The runner of `model`, one of [`MODELS`].
#[cfg(feature = "sounds")]
pub fn runner(model: &Model) -> Box<dyn BeatTracker> {
    match model.id {
        beat_this::ID => Box::<beat_this::BeatThis>::default(),
        id => unreachable!("{id} is not a beats model"),
    }
}
