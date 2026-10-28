//! Beat This! (MIT, CPJKU), its `final0` checkpoint: logits for every
//! 20 ms frame of up to 30 s at a time, read as Beat This!'s own "minimal"
//! postprocessor does (peaks above 0.5 within 70 ms, neighbours averaged,
//! downbeats moved onto beats).

pub mod config;
#[cfg(feature = "sounds")]
mod runner;

use crate::Model;
#[cfg(feature = "sounds")]
pub use runner::BeatThis;

pub const ID: &str = "beat-this-final0";

pub const MODEL: Model = Model {
    id: ID,
    name: "Beat This!",
    files: &[config::FILE],
};
