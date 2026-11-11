//! Speech enhancement: the voice in a recording kept, the noise, hum and
//! room around it taken away, for Clean voice. Not part of analysis.

pub mod deepfilternet;

use crate::Model;
use anyhow::Result;

/// The models that can do it, the default first.
pub const MODELS: &[&Model] = &[&deepfilternet::MODEL];

/// Samples per second of the sound the models hear and give back.
pub const RATE: u32 = 48_000;

/// Enhances speech in sound of one channel at [`RATE`].
pub trait Enhancer: Send + Sync {
    /// A pass over one stream, from its first sample.
    fn start(&self) -> Result<Box<dyn Pass + '_>>;
}

/// One stream enhanced in order. What comes out lags what goes in (the
/// model looks ahead and runs on stretches of several seconds); once the
/// input ends, [`Pass::finish`] gives the rest, so the output is as long as
/// the input, sample for sample at the same times.
pub trait Pass: Send {
    /// Take the next `samples`; the enhanced samples ready so far.
    fn push(&mut self, samples: &[f32]) -> Result<Vec<f32>>;
    /// The input has ended: the enhanced samples still owed.
    fn finish(self: Box<Self>) -> Result<Vec<f32>>;
}

/// The runner of `model`, one of [`MODELS`].
#[cfg(feature = "enhance")]
pub fn runner(model: &Model) -> Box<dyn Enhancer> {
    match model.id {
        deepfilternet::ID => Box::<deepfilternet::DeepFilterNet>::default(),
        id => unreachable!("{id} is not an enhancement model this build runs"),
    }
}
