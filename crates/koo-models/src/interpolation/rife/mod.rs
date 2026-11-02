//! RIFE (Practical-RIFE, MIT, hzwer; ECCV 2022 RIFE, Megvii, MIT): IFNet
//! v4 as FuryTMP's ONNX export, its time step made an input by walterlow,
//! so one run makes the frame at any moment between two.

pub mod config;
#[cfg(feature = "interpolate")]
mod runner;

use crate::Model;
#[cfg(feature = "interpolate")]
pub use runner::Rife;

pub const ID: &str = "rife-v4-fp32";

pub const MODEL: Model = Model {
    id: ID,
    name: "RIFE",
    files: &[config::FILE],
};
