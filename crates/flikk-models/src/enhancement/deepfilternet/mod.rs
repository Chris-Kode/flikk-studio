//! DeepFilterNet 3 (Schröter et al.; code and weights MIT or Apache-2.0):
//! full-band speech enhancement at 48 kHz. A gain per ERB band over the
//! whole spectrum, then a five-frame complex filter (deep filtering) over
//! the bands below 4.8 kHz, from three ONNX graphs published with the
//! model; the STFT, features and filtering around them are written here
//! after the authors' `libDF`.

pub mod config;
#[cfg(feature = "enhance")]
mod runner;

use crate::Model;
#[cfg(feature = "enhance")]
pub use runner::DeepFilterNet;

pub const ID: &str = "deepfilternet3";

pub const MODEL: Model = Model {
    id: ID,
    name: "DeepFilterNet 3",
    files: &[config::ARCHIVE],
};
