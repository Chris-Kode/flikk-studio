//! Interfaces implemented by `koo-media` and `koo-analysis`, so the UI and the
//! agent host can be written against them independently.

use crate::analysis::{Analysis, Loudness};
use crate::fx::{FxAsset, ParamValue};
use crate::ids::{JobId, MediaId};
use crate::model::{BlendMode, Fingerprint, MediaInfo, MediaItem};
use crate::time::{Time, TimeRange};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub type ServiceResult<T> = std::result::Result<T, ServiceError>;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ServiceError(pub String);

impl ServiceError {
    pub fn new(e: impl std::fmt::Display) -> Self {
        Self(e.to_string())
    }
}

/// Decoded RGBA8 image, straight (not premultiplied) alpha.
#[derive(Clone, Debug)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<[u8]>,
}

/// One tile of a collage, resolved to a source frame by the caller.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SamplePoint {
    pub timeline: Time,
    pub media: MediaId,
    pub path: PathBuf,
    pub fingerprint: Option<Fingerprint>,
    pub source: Time,
}

/// At most 30 tiles in a 6 × 5 grid (PRD V2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CollageRequest {
    pub samples: Vec<SamplePoint>,
    pub tile_width: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Collage {
    /// PNG, cached by fingerprint and range (PRD V6).
    pub path: PathBuf,
    pub tiles: Vec<SamplePoint>,
    pub columns: u32,
    pub rows: u32,
}

/// Sequential decode for playback, from a long-lived decoder process.
pub trait FrameStream: Send {
    /// Next frame and its source time; `None` at the end of the media.
    fn next_frame(&mut self) -> ServiceResult<Option<(Time, Frame)>>;
}

/// Decoded audio: interleaved stereo `f32` at the requested sample rate.
pub trait PcmStream: Send {
    /// Fill `out`, returning samples written; 0 at the end of the media.
    fn read(&mut self, out: &mut [f32]) -> ServiceResult<usize>;
}

pub trait MediaService: Send + Sync {
    fn probe(&self, path: &Path) -> ServiceResult<MediaInfo>;
    fn fingerprint(&self, path: &Path) -> ServiceResult<Fingerprint>;
    /// One frame of `item` at source time `t`, scaled to fit `max_width`.
    /// Uses the proxy when one exists.
    fn frame(&self, item: &MediaItem, t: Time, max_width: u32) -> ServiceResult<Frame>;
    /// Frames from source time `from` onward, for real-time playback.
    fn playback(
        &self,
        item: &MediaItem,
        from: Time,
        max_width: u32,
    ) -> ServiceResult<Box<dyn FrameStream>>;
    /// Audio of `stream` from source time `from` onward.
    fn audio(
        &self,
        item: &MediaItem,
        stream: u32,
        from: Time,
        sample_rate: u32,
    ) -> ServiceResult<Box<dyn PcmStream>>;
    fn collage(&self, request: &CollageRequest) -> ServiceResult<Collage>;
    /// Peak envelope for drawing, `buckets` values in 0..1 over `range`.
    fn waveform(
        &self,
        item: &MediaItem,
        stream: u32,
        range: TimeRange,
        buckets: usize,
    ) -> ServiceResult<Vec<f32>>;
    /// Create the playback proxy if missing; long-running.
    fn ensure_proxy(&self, item: &MediaItem, progress: &dyn Fn(f32)) -> ServiceResult<PathBuf>;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running { fraction: f32 },
    Done,
    Failed { message: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JobStatus {
    pub id: JobId,
    pub label: String,
    pub media: Option<MediaId>,
    pub state: JobState,
}

pub trait AnalysisService: Send + Sync {
    /// Queue transcript, voice activity, speakers and shot changes for an
    /// item; a no-op if cached for its fingerprint.
    fn analyze(&self, item: &MediaItem) -> JobId;
    fn jobs(&self) -> Vec<JobStatus>;
    /// Cached results, if analysis has run.
    fn analysis(&self, fingerprint: &Fingerprint) -> Option<Arc<Analysis>>;
    /// Gaps without speech longer than `min`, in source time (PRD E4).
    fn silences(&self, item: &MediaItem, min: Time) -> ServiceResult<Vec<TimeRange>>;
    fn loudness(&self, path: &Path, range: Option<TimeRange>) -> ServiceResult<Loudness>;
}

/// One evaluated FX program: what to run, its parameter values and clock.
/// Build `values` with [`crate::fx::FxInstance::values_at`].
#[derive(Clone, Copy, Debug)]
pub struct FxCall<'a> {
    pub asset: &'a FxAsset,
    pub values: &'a BTreeMap<String, ParamValue>,
    /// Seconds since the clip or transition started (`iTime`), and its
    /// length (`iDuration`). A transition's `progress` is `time / duration`.
    pub time: f64,
    pub duration: f64,
    /// Sequence size: HTML lays out at this size in CSS px.
    pub canvas: (u32, u32),
}

/// GLSL programs on the GPU (`koo-gpu`). Thread-safe; frames in and out are
/// RGBA8 with straight alpha.
pub trait FxService: Send + Sync {
    /// Compile `asset`. Errors name lines of the author's code, so the agent
    /// and the code editor can point at them.
    fn check(&self, asset: &FxAsset) -> ServiceResult<()>;
    /// Run `chain` (effects) in order over `frame`; same size out.
    fn apply(&self, frame: &Frame, chain: &[FxCall<'_>]) -> ServiceResult<Frame>;
    /// Blend `from` into `to` (same size) with a transition program at
    /// `call.time / call.duration`.
    fn transition(&self, from: &Frame, to: &Frame, call: &FxCall<'_>) -> ServiceResult<Frame>;
    /// Render a generator program at `width` × `height`.
    fn generate(&self, call: &FxCall<'_>, width: u32, height: u32) -> ServiceResult<Frame>;
    /// Draw `layers` bottom-up onto a `width` × `height` canvas filled with
    /// `background` (straight RGBA8). The default is the CPU reference,
    /// [`crate::compose::composite_cpu`]; engines must match it.
    fn composite(
        &self,
        width: u32,
        height: u32,
        background: [u8; 4],
        layers: &[CompositeLayer<'_>],
    ) -> ServiceResult<Frame> {
        Ok(crate::compose::composite_cpu(
            width, height, background, layers,
        ))
    }
}

/// Where a picture lands on a canvas, in canvas pixels (y down).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// Centre of the picture.
    pub center: [f32; 2],
    /// Drawn size of the whole (uncropped) picture before rotation.
    pub size: [f32; 2],
    /// Clockwise, around the centre.
    pub rotation_deg: f32,
    /// Fractions of the picture cut away, [left, top, right, bottom], in
    /// the picture's own (unflipped) coordinates.
    pub crop: [f32; 4],
    /// Mirror horizontally / vertically.
    pub flip: [bool; 2],
    pub opacity: f32,
    pub blend: BlendMode,
}

impl Placement {
    /// The picture stretched over a whole `width` × `height` canvas.
    pub fn fill(width: u32, height: u32) -> Self {
        Self {
            center: [width as f32 / 2.0, height as f32 / 2.0],
            size: [width as f32, height as f32],
            rotation_deg: 0.0,
            crop: [0.0; 4],
            flip: [false; 2],
            opacity: 1.0,
            blend: BlendMode::Normal,
        }
    }
}

/// One picture for [`FxService::composite`]: sampled bilinearly with
/// premultiplied alpha, transparent outside the picture and its crop, then
/// blended per [`BlendMode`].
#[derive(Clone, Copy, Debug)]
pub struct CompositeLayer<'a> {
    pub frame: &'a Frame,
    pub placement: Placement,
}

/// HTML generators in a headless browser (`koo-html`).
pub trait HtmlService: Send + Sync {
    /// Load the document at `call.time` and return script and console errors
    /// (empty when clean).
    fn check(&self, call: &FxCall<'_>) -> ServiceResult<Vec<String>>;
    /// One frame with alpha, laid out at `call.canvas` and scaled to
    /// `width` × `height`.
    fn frame(&self, call: &FxCall<'_>, width: u32, height: u32) -> ServiceResult<Frame>;
}

/// The engines the UI and agent host call. `None` until wired, or when the
/// machine lacks them (no GPU adapter, no Chromium); callers degrade quietly.
#[derive(Clone, Default)]
pub struct Services {
    pub media: Option<Arc<dyn MediaService>>,
    pub analysis: Option<Arc<dyn AnalysisService>>,
    pub fx: Option<Arc<dyn FxService>>,
    pub html: Option<Arc<dyn HtmlService>>,
}

impl std::fmt::Debug for Services {
    /// Which engines are wired.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Services")
            .field("media", &self.media.is_some())
            .field("analysis", &self.analysis.is_some())
            .field("fx", &self.fx.is_some())
            .field("html", &self.html.is_some())
            .finish()
    }
}
