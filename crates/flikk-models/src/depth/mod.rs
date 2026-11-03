//! Depth: how far each part of a picture is from the camera, relative to
//! the rest of it, for the depth-aware effects (lens blur, fog, parallax)
//! and depth masks. Not part of analysis.

pub mod depth_anything;

use crate::Model;
use anyhow::Result;
use flikk_core::services::Frame;

/// The models that can do it, the default first.
pub const MODELS: &[&Model] = &[&depth_anything::MODEL];

/// A picture's relative depth as the model gives it: disparity (larger is
/// nearer) up to an unknown scale and shift, row by row, top row first.
#[derive(Clone, Debug)]
pub struct Disparity {
    pub width: usize,
    pub height: usize,
    pub values: Vec<f32>,
}

/// Estimates depth in single pictures.
pub trait DepthEstimator: Send + Sync {
    /// The relative depth of `picture` (any size; the model sees it at its
    /// own, smaller one, keeping its shape).
    fn estimate(&self, picture: &Frame) -> Result<Disparity>;
}

/// The runner of `model`, one of [`MODELS`].
#[cfg(feature = "depth")]
pub fn runner(model: &Model) -> Box<dyn DepthEstimator> {
    match model.id {
        depth_anything::ID => Box::<depth_anything::DepthAnything>::default(),
        id => unreachable!("{id} is not a depth model this build runs"),
    }
}
