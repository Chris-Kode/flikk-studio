//! LAION's larger_clap_general (Apache-2.0): its audio tower turns 4 s
//! windows every 2 s into unit vectors, its text tower turns a description
//! into one. The audio tower runs in analysis, the text tower when a sound
//! is looked for by description.

pub mod config;
#[cfg(feature = "sounds")]
mod runner;

use crate::Model;
#[cfg(feature = "sounds")]
pub use runner::Clap;

pub const ID: &str = "larger-clap-general";

pub const MODEL: Model = Model {
    id: ID,
    name: "LAION CLAP",
    files: &[config::AUDIO, config::TEXT, config::VOCAB, config::MERGES],
};
