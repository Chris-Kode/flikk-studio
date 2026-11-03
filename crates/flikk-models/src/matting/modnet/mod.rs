//! MODNet (Apache-2.0, Ke et al., "MODNet: Trimap-Free Portrait Matting in
//! Real Time"): a MobileNetV2 network that finds the people in a picture
//! and their soft edges in one pass, without a trimap. Run as one ONNX
//! graph on frames at a reduced size; the full-size edge is the mask's
//! refinement (`flikk_core::person`).

pub mod config;
#[cfg(feature = "matting")]
mod runner;

use crate::Model;
#[cfg(feature = "matting")]
pub use runner::Modnet;

pub const ID: &str = "modnet-photographic";

pub const MODEL: Model = Model {
    id: ID,
    name: "MODNet",
    files: &[config::GRAPH],
};
