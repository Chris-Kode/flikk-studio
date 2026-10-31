//! SAM 2.1 tiny (Apache-2.0, Meta): a picture encoder whose features a
//! decoder turns into the object clicks and a box pick, and memory modules
//! that follow it through the frames after (and before) them, through
//! occlusion. Run as five ONNX graphs, the memory assembled here the way
//! `transformers`' `Sam2VideoModel` does.

pub mod config;
#[cfg(feature = "segment")]
mod runner;

use crate::Model;
#[cfg(feature = "segment")]
pub use runner::Sam2;

pub const ID: &str = "sam2.1-hiera-tiny";

pub const MODEL: Model = Model {
    id: ID,
    name: "SAM 2.1 tiny",
    files: &[
        config::ENCODER,
        config::DECODER,
        config::MEMORY_ENCODER,
        config::MEMORY_ATTENTION,
        config::POINTER_POSITIONS,
        config::CONSTANTS,
    ],
};
