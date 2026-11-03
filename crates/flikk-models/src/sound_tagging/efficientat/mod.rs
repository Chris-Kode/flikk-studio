//! Sound tagging with EfficientAT mn10_as (MIT, Florian Schmid): the 527
//! AudioSet classes for 2 s windows every second of 32 kHz audio, turned
//! into the sounds and music of the sound script.
//!
//! The model was trained on 10 s clips and answers nonsense for 2 s alone;
//! followed by 4 s of silence, a window gets nine in ten of the answers it
//! would get padded to 10 s, at half the cost. A window is tagged with a
//! sound wherever in it the sound plays, so a sound's run of windows
//! starts up to 2 s before it and ends up to 1 s before its end; the run
//! is shrunk by that much, and a short sound is then moved onto the
//! loudest stretch of the audio levels around it.

pub mod config;
#[cfg(feature = "sounds")]
mod groups;
#[cfg(feature = "sounds")]
mod runner;

use crate::Model;
#[cfg(feature = "sounds")]
pub use runner::EfficientAt;

pub const ID: &str = "efficientat-mn10-as";

pub const MODEL: Model = Model {
    id: ID,
    name: "EfficientAT mn10",
    files: &[config::FILE],
};
