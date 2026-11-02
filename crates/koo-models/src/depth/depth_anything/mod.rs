//! Depth Anything V2 Small (Apache-2.0; the Base and Large weights are
//! CC-BY-NC and are not used): a DINOv2 ViT-S encoder and a DPT head giving
//! relative disparity for any picture, as onnx-community's ONNX export.

pub mod config;
#[cfg(feature = "depth")]
mod runner;

use crate::Model;
#[cfg(feature = "depth")]
pub use runner::DepthAnything;

pub const ID: &str = "depth-anything-v2-small";

pub const MODEL: Model = Model {
    id: ID,
    name: "Depth Anything V2 Small",
    files: &[config::MODEL_FILE],
};
