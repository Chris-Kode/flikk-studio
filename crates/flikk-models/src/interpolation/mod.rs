//! Frame interpolation: a frame between two neighbouring video frames,
//! along the motion, for smooth slow motion.

pub mod rife;

use crate::Model;
use anyhow::Result;
use flikk_core::services::Frame;
use flikk_os::gpu;

/// The models that can do it, the default first.
pub const MODELS: &[&Model] = &[&rife::MODEL];

/// Makes frames between frames.
pub trait Interpolator: Send + Sync {
    /// The frame `at` (0..1) of the way from `a` to `b`, which are the
    /// same size; RGBA, opaque.
    /// On the GPU it runs in `unit`'s turn (`flikk_os::gpu`), and fails
    /// with "canceled" once that is cancelled while it waits.
    fn between(&self, a: &Frame, b: &Frame, at: f32, unit: gpu::Unit<'_>) -> Result<Frame>;

    /// Whether [`Self::between`] runs on the GPU now, and takes turns.
    fn on_gpu(&self) -> bool;
}

/// The runner of `model`, one of [`MODELS`].
#[cfg(feature = "interpolate")]
pub fn runner(model: &Model) -> Box<dyn Interpolator> {
    match model.id {
        rife::ID => Box::<rife::Rife>::default(),
        id => unreachable!("{id} is not a frame interpolation model"),
    }
}
