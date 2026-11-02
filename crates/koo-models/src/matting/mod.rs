//! Matting: how much of each pixel of a picture is a person, soft at hair
//! and motion blur, with no prompt and no green screen. Person masks use
//! it; subjects that are not people are segmentation's.

pub mod modnet;

use crate::Model;
use anyhow::Result;
use koo_core::services::Frame;

/// The models that can do it, the default first.
pub const MODELS: &[&Model] = &[&modnet::MODEL];

/// The people in one picture: `width` × `height` coverages 0..1 over the
/// whole picture (its proportions, at the model's size), row by row.
#[derive(Clone, Debug)]
pub struct Alpha {
    pub width: usize,
    pub height: usize,
    pub values: Vec<f32>,
}

/// Finds the people in pictures.
pub trait Matter: Send + Sync {
    /// The people in `picture` (any size; resampled to the model's).
    fn matte(&self, picture: &Frame) -> Result<Alpha>;
}

/// The runner of `model`, one of [`MODELS`].
#[cfg(feature = "matting")]
pub fn runner(model: &Model) -> Box<dyn Matter> {
    match model.id {
        modnet::ID => Box::<modnet::Modnet>::default(),
        id => unreachable!("{id} is not a matting model this build runs"),
    }
}
