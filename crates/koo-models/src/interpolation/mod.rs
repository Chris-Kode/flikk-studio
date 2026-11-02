//! Frame interpolation: a frame between two neighbouring video frames,
//! along the motion, for smooth slow motion.

pub mod rife;

use crate::Model;
use anyhow::Result;
use koo_core::services::Frame;

/// The models that can do it, the default first.
pub const MODELS: &[&Model] = &[&rife::MODEL];

/// Makes frames between frames.
pub trait Interpolator: Send + Sync {
    /// The frame `at` (0..1) of the way from `a` to `b`, which are the
    /// same size; RGBA, opaque.
    fn between(&self, a: &Frame, b: &Frame, at: f32) -> Result<Frame>;
}

/// The runner of `model`, one of [`MODELS`].
#[cfg(feature = "interpolate")]
pub fn runner(model: &Model) -> Box<dyn Interpolator> {
    match model.id {
        rife::ID => Box::<rife::Rife>::default(),
        id => unreachable!("{id} is not a frame interpolation model"),
    }
}
