//! Interfaces implemented by `koo-media` and `koo-analysis`, so the UI and the
//! agent host can be written against them independently.

use crate::analysis::{Analysis, Loudness};
use crate::ids::{JobId, MediaId};
use crate::model::{Fingerprint, MediaInfo, MediaItem};
use crate::time::{Time, TimeRange};
use serde::{Deserialize, Serialize};
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

/// Decoded RGBA8 image.
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

pub trait MediaService: Send + Sync {
    fn probe(&self, path: &Path) -> ServiceResult<MediaInfo>;
    fn fingerprint(&self, path: &Path) -> ServiceResult<Fingerprint>;
    /// One frame of `item` at source time `t`, scaled to fit `max_width`.
    /// Uses the proxy when one exists.
    fn frame(&self, item: &MediaItem, t: Time, max_width: u32) -> ServiceResult<Frame>;
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

/// The engines the UI and agent host call. `None` until wired.
#[derive(Clone, Default)]
pub struct Services {
    pub media: Option<Arc<dyn MediaService>>,
    pub analysis: Option<Arc<dyn AnalysisService>>,
}
