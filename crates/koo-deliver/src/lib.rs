//! Deliver: render the timeline with FFmpeg, export SRT/ASS captions and
//! OpenTimelineIO / FCPXML.
//!
//! Everything here works on a [`Project`](koo_core::model::Project) snapshot,
//! so callers clone the project on the UI thread and hand it to a background
//! thread. FFmpeg is spawned as a binary (`KOOEDITOR_FFMPEG`, else `PATH`).

mod audio_fx;
pub mod captions;
mod composed;
pub mod fcpxml;
pub mod ffmpeg;
pub mod frame;
pub mod grade;
pub mod graph;
pub mod keyframes;
pub mod lettering;
pub mod otio;
pub mod preset;
pub mod queue;

pub use captions::{Cue, export_ass, export_srt};
pub use fcpxml::export_fcpxml;
pub use ffmpeg::free_name;
pub use frame::export_frame;
pub use lettering::Letterer;
pub use otio::export_otio;
pub use preset::{AudioCodec, Container, LoudnessTarget, RenderSettings, VideoCodec};
pub use queue::{RenderJob, RenderQueue, RenderRequest, RenderState, render_blocking};
