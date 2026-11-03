//! SAM 2.1 tiny (Apache-2.0, Meta) on MLX, Apple's array framework: the
//! same model as `sam2`, computed on the Mac's GPU in its unified memory.
//! The network is written here over `mlx-rs` from SAM 2's own modules and
//! its tracking as SAM 2 does it, with every prompted frame in memory (the
//! ONNX graphs take a fixed-size bank).

pub mod config;
#[cfg(feature = "mlx")]
mod net;
#[cfg(feature = "mlx")]
mod runner;

use crate::Model;
#[cfg(feature = "mlx")]
pub use runner::Sam2Mlx;

pub const ID: &str = "sam2.1-hiera-tiny-mlx";

pub const MODEL: Model = Model {
    id: ID,
    name: "SAM 2.1 tiny (MLX)",
    files: &[config::WEIGHTS],
};
