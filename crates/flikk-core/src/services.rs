//! Interfaces implemented by `flikk-media` and `flikk-analysis`, so the UI and the
//! agent host can be written against them independently.

use crate::analysis::{Analysis, AnalysisSteps, Loudness, SoundVectors};
use crate::cloud::SpeechService;
use crate::fx::{FxAsset, ParamValue};
use crate::ids::{JobId, MaskId, MediaId};
use crate::model::{BlendMode, Clip, Fingerprint, MediaInfo, MediaItem, Track};
use crate::person::PersonMatte;
use crate::subject::{Matte, Subject, SubjectPrompt};
use crate::time::{Time, TimeRange};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

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

/// A picture of the audio heard over a timeline range, for the agent's
/// `view_audio`: a spectrogram lane per track, with marks under it.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioPictureRequest {
    /// Timeline range across the picture.
    pub range: TimeRange,
    pub lanes: Vec<AudioLane>,
    pub marks: Vec<AudioMark>,
    pub width: u32,
}

/// One track's audio: the stretches of source it plays.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioLane {
    pub name: String,
    pub segments: Vec<AudioSegment>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AudioSegment {
    pub path: PathBuf,
    /// Position in `MediaInfo::audio`.
    pub stream: u32,
    pub source: TimeRange,
    pub timeline: TimeRange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioMarkKind {
    Word,
    Sound,
    Beat,
    Downbeat,
}

/// Something drawn under the lanes at its timeline range.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioMark {
    pub at: TimeRange,
    pub kind: AudioMarkKind,
    pub label: String,
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
    /// Draw the audio of a timeline range (see [`AudioPictureRequest`]).
    fn audio_picture(&self, _request: &AudioPictureRequest) -> ServiceResult<Frame> {
        Err(ServiceError(
            "Audio pictures are not available in this build.".into(),
        ))
    }
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

/// How analysis runs from the next item on: its steps, and who
/// transcribes.
#[derive(Clone, Default)]
pub struct AnalysisConfig {
    pub steps: AnalysisSteps,
    /// Transcribes, with speakers, in place of the local speech models;
    /// `None` runs those.
    pub speech: Option<Arc<dyn SpeechService>>,
}

/// How much of what [`AnalysisService::analyze`] would make an item's
/// cached results hold.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Coverage {
    /// Not analyzed.
    #[default]
    None,
    /// Some steps are missing; analyzing runs only those.
    Partial,
    /// Everything; only [`AnalysisService::analyze_again`] runs anything.
    Full,
}

pub trait AnalysisService: Send + Sync {
    /// Queue transcript, voice activity, speakers and shot changes for an
    /// item; a no-op if its cached result covers the steps configured now,
    /// else only the missing ones run.
    fn analyze(&self, item: &MediaItem) -> JobId;
    /// Queue what is heard in a library sound: sounds, music, beats and
    /// sound vectors, on this machine. No speech, so it spends no
    /// transcription credits. Nothing if it has results, or the steps or
    /// their models are off. May read the disk.
    fn analyze_sound(&self, _item: &MediaItem) {}
    /// Apply `config` to the items analyzed from now on; a job already
    /// running keeps the one it started with.
    fn configure(&self, _config: AnalysisConfig) {}
    /// Like [`AnalysisService::analyze`], but replacing the cached
    /// results, on the models picked now.
    fn analyze_again(&self, item: &MediaItem) -> JobId {
        self.analyze(item)
    }
    fn jobs(&self) -> Vec<JobStatus>;
    /// Cached results, if analysis has run.
    fn analysis(&self, fingerprint: &Fingerprint) -> Option<Arc<Analysis>>;
    /// How much of the steps configured now `item`'s cached results cover.
    /// Cheap enough to ask while drawing. A service that does not track
    /// steps takes any result for a whole one.
    fn coverage(&self, item: &MediaItem) -> Coverage {
        match item.fingerprint.as_ref().and_then(|fp| self.analysis(fp)) {
            Some(_) => Coverage::Full,
            None => Coverage::None,
        }
    }
    /// Gaps without speech longer than `min`, in source time (PRD E4).
    fn silences(&self, item: &MediaItem, min: Time) -> ServiceResult<Vec<TimeRange>>;
    fn loudness(&self, path: &Path, range: Option<TimeRange>) -> ServiceResult<Loudness>;
    /// What `fingerprint`'s audio sounds like, for finding a sound by
    /// description, once analysis has made them.
    fn sound_vectors(&self, _fingerprint: &Fingerprint) -> Option<Arc<SoundVectors>> {
        None
    }
    /// The vector of a description of a sound, to score against
    /// [`AnalysisService::sound_vectors`]. Slow the first time: it loads
    /// (and may download) a model.
    fn describe_sound(&self, _text: &str) -> ServiceResult<Vec<f32>> {
        Err(ServiceError(
            "Sound search is not available in this build.".into(),
        ))
    }
    /// The words of `script`, said in the file at `path`, with where each
    /// is said there: as a voiceover's words are timed. Slow: it decodes
    /// the file and runs the aligner when it is downloaded.
    fn time_script(
        &self,
        _path: &Path,
        _script: &str,
    ) -> ServiceResult<Vec<crate::analysis::Word>> {
        Err(ServiceError(
            "Timing a script is not available in this build.".into(),
        ))
    }
}

/// A subject mask as [`SubjectService`] sees it: the media under it, and
/// the part of it its clip shows.
#[derive(Clone, Copy, Debug)]
pub struct SubjectRef<'a> {
    /// Its tracking is replaced, not added to, when the subject changes.
    pub mask: MaskId,
    pub item: &'a MediaItem,
    pub subject: &'a Subject,
    /// Source range.
    pub range: TimeRange,
}

/// A person mask as [`SubjectService`] sees it: the media under it, and
/// the part of it its clip shows.
#[derive(Clone, Copy, Debug)]
pub struct PersonRef<'a> {
    pub mask: MaskId,
    pub item: &'a MediaItem,
    /// Source range.
    pub range: TimeRange,
}

/// How far a subject's mattes are made.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SubjectStatus {
    /// Prompted frames made, of all.
    pub prompted: (usize, usize),
    /// Frames to track over the clip (with the prompted ones), and how
    /// many are.
    pub frames: usize,
    pub tracked: usize,
    /// Tracking is running (or waiting its turn).
    pub tracking: bool,
    /// Seconds a frame took to track lately on this machine.
    pub seconds_per_frame: Option<f32>,
    /// Why making them failed.
    pub error: Option<String>,
}

impl SubjectStatus {
    /// Every matte the subject shows is made.
    pub fn complete(&self, track: bool) -> bool {
        self.prompted.0 == self.prompted.1 && (!track || self.tracked >= self.frames)
    }
}

/// Subject masks (`flikk-analysis`): the object a subject picks, as a
/// segmentation model finds it on the prompted frames and follows it
/// through its clip. Mattes are kept by fingerprint and prompts, so a
/// subject is found once.
pub trait SubjectService: Send + Sync {
    /// Whether the model is here; why not otherwise.
    fn available(&self) -> Result<(), String>;
    /// The subject's matte at source time `t`: that frame's when it is
    /// made, else the nearest made one (a prompted frame without
    /// tracking). Starts making what is missing (the prompted frames, and
    /// with `track` every frame of the range), so a later call may find
    /// more. `None` until one is made.
    fn matte(&self, subject: SubjectRef<'_>, t: Time) -> Option<Matte>;
    fn status(&self, subject: SubjectRef<'_>) -> SubjectStatus;
    /// Make everything [`SubjectService::matte`] shows for the subject,
    /// waiting for it (renders). `progress` gets 0..1.
    fn complete(
        &self,
        subject: SubjectRef<'_>,
        progress: &dyn Fn(f32),
        cancel: &AtomicBool,
    ) -> ServiceResult<()>;
    /// The object `prompt` picks on its frame of `item`, by itself (what
    /// the subject shows there once the prompt is in it). Slow the first
    /// time a frame is asked about, which is encoded; fast after.
    fn preview(&self, item: &MediaItem, prompt: &SubjectPrompt) -> ServiceResult<Matte>;
    /// Counts mattes made, so views know to draw again.
    fn version(&self) -> u64;

    /// Whether person masks can be made (their model is here); why not
    /// otherwise.
    fn people_available(&self) -> Result<(), String> {
        Err("Person masks are not in this build.".into())
    }
    /// The people at source time `t` of a person mask's media: that
    /// frame's matte when it is made, else the nearest made within a
    /// second. Starts making the frames from `t` on (a few seconds ahead,
    /// for playback), so a later call may find more. `None` until one is
    /// made.
    fn person(&self, _person: PersonRef<'_>, _t: Time) -> Option<PersonMatte> {
        None
    }
    /// How far the frames of a person mask's range are made (`frames`
    /// and `tracked`; `tracking` while some are being made).
    fn person_status(&self, _person: PersonRef<'_>) -> SubjectStatus {
        SubjectStatus {
            error: self.people_available().err(),
            ..Default::default()
        }
    }
    /// Make every frame of a person mask's range, waiting for it
    /// (renders). `progress` gets 0..1.
    fn complete_person(
        &self,
        _person: PersonRef<'_>,
        _progress: &dyn Fn(f32),
        _cancel: &AtomicBool,
    ) -> ServiceResult<()> {
        Err(ServiceError(self.people_available().err().unwrap_or_else(
            || "Person masks are not in this build.".into(),
        )))
    }
}

/// Two neighbouring source frames of a video and a moment between them,
/// for [`InterpolationService::between`].
#[derive(Clone, Copy, Debug)]
pub struct Between<'a> {
    pub item: &'a MediaItem,
    /// The earlier frame and its source time.
    pub a: (Time, &'a Frame),
    /// The next one and its source time.
    pub b: (Time, &'a Frame),
    /// How far from `a` to `b`, 0..1, in steps of
    /// [`crate::speed::BETWEEN_STEPS`].
    pub at: f32,
}

/// Frames made between two source frames along the motion (optical flow,
/// `flikk-analysis`), kept by fingerprint, frames, moment and size.
pub trait InterpolationService: Send + Sync {
    /// Whether the model is here; why not otherwise.
    fn available(&self) -> Result<(), String>;
    /// The frame between, when it is made. Otherwise starts making it and
    /// returns `None`, or with `wait` (renders) makes it now; `None` when
    /// it cannot be made.
    fn between(&self, request: &Between<'_>, wait: bool) -> Option<Frame>;
    /// Counts frames made, so views know to draw again.
    fn version(&self) -> u64;
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
    /// Sequence size: HTML lays out in its shape (see [`crate::fx`]), and
    /// GLSL programs measure `iResolution` in its pixels.
    pub canvas: (u32, u32),
}

/// GLSL programs on the GPU (`flikk-gpu`). Thread-safe; frames in and out are
/// RGBA8 with straight alpha.
///
/// Programs see `iResolution` as the frame's size fitted inside
/// [`FxCall::canvas`], whatever size they actually render at, so a radius in
/// pixels looks the same in a small preview and in a full-size render.
pub trait FxService: Send + Sync {
    /// Compile `asset`. Errors name lines of the author's code, so the agent
    /// and the code editor can point at them.
    fn check(&self, asset: &FxAsset) -> ServiceResult<()>;
    /// A batch for one composed frame: pictures stay on the GPU between its
    /// steps and come back once.
    fn batch(&self) -> Box<dyn FxBatch + '_>;
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

/// A picture held by an [`FxBatch`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Surface(pub usize);

/// Byte order of a returned frame's pixels. With `Bgra`, [`Frame::rgba`]
/// holds blue, green, red, alpha: the order GPUI images take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelOrder {
    Rgba,
    Bgra,
}

/// A frame on its way back from an engine ([`FxBatch::submit`]).
pub trait PendingFrame {
    /// Block until the picture is back.
    fn wait(self: Box<Self>) -> ServiceResult<Frame>;
}

/// A frame that is already there.
impl PendingFrame for Frame {
    fn wait(self: Box<Self>) -> ServiceResult<Frame> {
        Ok(*self)
    }
}

/// The steps of one composed frame, kept where the engine runs them: frames
/// go in with [`FxBatch::upload`], each step returns a new [`Surface`], and
/// [`FxBatch::submit`] starts them all and returns one picture. Steps mean
/// what the [`FxService`] methods of the same name mean; errors from a step
/// leave the batch usable.
pub trait FxBatch {
    fn upload(&mut self, frame: &Frame) -> ServiceResult<Surface>;
    fn size(&self, surface: Surface) -> (u32, u32);
    fn apply(&mut self, input: Surface, chain: &[FxCall<'_>]) -> ServiceResult<Surface>;
    /// [`FxBatch::apply`] with `depth` (an uploaded [`DepthMap::to_frame`],
    /// any size) bound for the programs that read it
    /// ([`crate::depth::needs_depth`]). Engines without depth run the chain
    /// as it is.
    ///
    /// [`DepthMap::to_frame`]: crate::depth::DepthMap::to_frame
    fn apply_depth(
        &mut self,
        input: Surface,
        depth: Surface,
        chain: &[FxCall<'_>],
    ) -> ServiceResult<Surface> {
        let _ = depth;
        self.apply(input, chain)
    }
    fn transition(
        &mut self,
        from: Surface,
        to: Surface,
        call: &FxCall<'_>,
    ) -> ServiceResult<Surface>;
    fn generate(&mut self, call: &FxCall<'_>, width: u32, height: u32) -> ServiceResult<Surface>;
    fn composite(
        &mut self,
        width: u32,
        height: u32,
        background: [u8; 4],
        layers: &[(Surface, Placement)],
    ) -> ServiceResult<Surface>;
    /// `picture` with its alpha multiplied by `mask`'s, sampled bilinearly
    /// at the same uv (any size), as [`crate::mask::apply`].
    fn mask(&mut self, picture: Surface, mask: Surface) -> ServiceResult<Surface>;
    /// Color multiplied by alpha.
    fn premultiply(&mut self, surface: Surface) -> ServiceResult<Surface>;
    /// Premultiplied color made straight again.
    fn unpremultiply(&mut self, surface: Surface) -> ServiceResult<Surface>;
    /// `from` → `to` at `progress` (a cross-dissolve of premultiplied
    /// pictures of the same size; `to`'s size wins otherwise).
    fn mix(&mut self, from: Surface, to: Surface, progress: f32) -> ServiceResult<Surface>;
    /// Start every step; `surface` comes back in `order` from
    /// [`PendingFrame::wait`], so the caller can prepare the next frame
    /// meanwhile.
    fn submit<'p>(
        self: Box<Self>,
        surface: Surface,
        order: PixelOrder,
    ) -> ServiceResult<Box<dyn PendingFrame + 'p>>
    where
        Self: 'p;
    /// Run every step and return `surface` (RGBA).
    fn finish(self: Box<Self>, surface: Surface) -> ServiceResult<Frame> {
        self.submit(surface, PixelOrder::Rgba)?.wait()
    }
}

/// Where a picture lands on a canvas, in canvas pixels (y down).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// Centre of the picture.
    pub center: [f32; 2],
    /// Drawn size of the whole (uncropped) picture before skew and rotation.
    pub size: [f32; 2],
    /// Clockwise, around the centre.
    pub rotation_deg: f32,
    /// Slant before rotation, [x, y] degrees, as [`Transform::skew_deg`].
    ///
    /// [`Transform::skew_deg`]: crate::model::Transform::skew_deg
    pub skew_deg: [f32; 2],
    /// Fractions of the picture cut away, [left, top, right, bottom], in
    /// the picture's own (unflipped) coordinates.
    pub crop: [f32; 4],
    /// Mirror horizontally / vertically.
    pub flip: [bool; 2],
    pub opacity: f32,
    pub blend: BlendMode,
    /// How far each corner of the picture moves before the rest of the
    /// placement, [top-left, top-right, bottom-right, bottom-left], in its
    /// uv, as [`Transform::corner_pin`]; all zero = none. Pinned, the
    /// picture maps onto the canvas through a homography instead of an
    /// affine map.
    ///
    /// [`Transform::corner_pin`]: crate::model::Transform::corner_pin
    pub pin: [[f32; 2]; 4],
}

/// A 3×3 matrix, row-major, acting on `(x, y, 1)`.
pub type Matrix3 = [f32; 9];

fn mul3(a: &Matrix3, b: &Matrix3) -> Matrix3 {
    let mut out = [0.0; 9];
    for r in 0..3 {
        for c in 0..3 {
            out[r * 3 + c] = (0..3).map(|k| a[r * 3 + k] * b[k * 3 + c]).sum();
        }
    }
    out
}

/// `m⁻¹`, if `m` is not singular.
fn invert3(m: &Matrix3) -> Option<Matrix3> {
    let [a, b, c, d, e, f, g, h, i] = *m;
    let co = [
        e * i - f * h,
        c * h - b * i,
        b * f - c * e,
        f * g - d * i,
        a * i - c * g,
        c * d - a * f,
        d * h - e * g,
        b * g - a * h,
        a * e - b * d,
    ];
    let det = a * co[0] + b * co[3] + c * co[6];
    if det.abs() < 1e-12 || !det.is_finite() {
        return None;
    }
    Some(co.map(|v| v / det))
}

/// `m` applied to `p`, divided through; `None` behind the projection.
fn project(m: &Matrix3, p: [f32; 2]) -> Option<[f32; 2]> {
    let w = m[6] * p[0] + m[7] * p[1] + m[8];
    if w <= 1e-9 {
        return None;
    }
    Some([
        (m[0] * p[0] + m[1] * p[1] + m[2]) / w,
        (m[3] * p[0] + m[4] * p[1] + m[5]) / w,
    ])
}

/// The homography taking the unit square's corners (top-left, top-right,
/// bottom-right, bottom-left) to `quad`'s (Heckbert's square-to-quad).
fn square_to_quad(quad: [[f32; 2]; 4]) -> Option<Matrix3> {
    let [[x0, y0], [x1, y1], [x2, y2], [x3, y3]] = quad;
    let (sx, sy) = (x0 - x1 + x2 - x3, y0 - y1 + y2 - y3);
    let (g, h) = if sx.abs() < 1e-9 && sy.abs() < 1e-9 {
        (0.0, 0.0)
    } else {
        let (dx1, dx2, dy1, dy2) = (x1 - x2, x3 - x2, y1 - y2, y3 - y2);
        let den = dx1 * dy2 - dx2 * dy1;
        if den.abs() < 1e-12 {
            return None;
        }
        ((sx * dy2 - dx2 * sy) / den, (dx1 * sy - sx * dy1) / den)
    };
    Some([
        x1 - x0 + g * x1,
        x3 - x0 + h * x3,
        x0,
        y1 - y0 + g * y1,
        y3 - y0 + h * y3,
        y0,
        g,
        h,
        1.0,
    ])
}

impl Placement {
    /// The picture stretched over a whole `width` × `height` canvas.
    pub fn fill(width: u32, height: u32) -> Self {
        Self {
            center: [width as f32 / 2.0, height as f32 / 2.0],
            size: [width as f32, height as f32],
            rotation_deg: 0.0,
            skew_deg: [0.0; 2],
            crop: [0.0; 4],
            flip: [false; 2],
            opacity: 1.0,
            blend: BlendMode::Normal,
            pin: [[0.0; 2]; 4],
        }
    }

    /// Whether any corner is pinned.
    pub fn is_pinned(&self) -> bool {
        self.pin.iter().flatten().any(|v| v.abs() > 1e-6)
    }

    /// Rotation after skew, `[a, b, c, d]`: a picture point `(x, y)` pixels
    /// from its centre lands at `center + (a·x + b·y, c·x + d·y)`.
    pub fn linear(&self) -> [f32; 4] {
        let (s, c) = self.rotation_deg.to_radians().sin_cos();
        let [kx, ky] = self.skew_deg.map(|d| d.to_radians().tan());
        [c - s * ky, c * kx - s, s + c * ky, s * kx + c]
    }

    /// Canvas point of `uv`, 0..1 across the whole picture.
    pub fn to_canvas(&self, uv: [f32; 2]) -> [f32; 2] {
        if self.is_pinned()
            && let Some(m) = self.homography()
            && let Some(p) = project(&m, uv)
        {
            return p;
        }
        self.unpinned_to_canvas(uv)
    }

    /// Where `uv` lands without the corner pin: the affine part of the
    /// placement, in which the pin's corners are measured.
    pub fn unpinned_to_canvas(&self, uv: [f32; 2]) -> [f32; 2] {
        let [a, b, c, d] = self.linear();
        let x = (uv[0] - 0.5) * self.size[0];
        let y = (uv[1] - 0.5) * self.size[1];
        [
            self.center[0] + a * x + b * y,
            self.center[1] + c * x + d * y,
        ]
    }

    /// Picture `uv` → canvas, the affine part only.
    fn affine(&self) -> Matrix3 {
        let [a, b, c, d] = self.linear();
        let [w, h] = self.size;
        let [cx, cy] = self.center;
        [
            a * w,
            b * h,
            cx - 0.5 * (a * w + b * h),
            c * w,
            d * h,
            cy - 0.5 * (c * w + d * h),
            0.0,
            0.0,
            1.0,
        ]
    }

    /// Picture `uv` → canvas, corner pin included. `None` when the pinned
    /// corners make no quad.
    pub fn homography(&self) -> Option<Matrix3> {
        let affine = self.affine();
        if !self.is_pinned() {
            return Some(affine);
        }
        let corners = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let quad = std::array::from_fn(|i| {
            [
                corners[i][0] + self.pin[i][0],
                corners[i][1] + self.pin[i][1],
            ]
        });
        Some(mul3(&affine, &square_to_quad(quad)?))
    }

    /// Canvas → picture `uv`, corner pin included, as a 3×3 matrix: `uv =
    /// (r₀·q, r₁·q) / (r₂·q)` for `q = (x, y, 1)`, outside the picture
    /// where `r₂·q ≤ 0`. Unpinned, the last row is `(0, 0, 1)` and this is
    /// [`Self::inverse`]. `None` when the picture is flat.
    pub fn uv_map(&self) -> Option<Matrix3> {
        if !self.is_pinned() {
            let [a, b, c, d] = self.inverse()?;
            let [cx, cy] = self.center;
            return Some([
                a,
                b,
                0.5 - a * cx - b * cy,
                c,
                d,
                0.5 - c * cx - d * cy,
                0.0,
                0.0,
                1.0,
            ]);
        }
        self.inverse()?;
        invert3(&self.homography()?)
    }

    /// Canvas → picture `uv` (0..1 across the whole picture), as `[a, b, c,
    /// d]`: `uv = (a·dx + b·dy, c·dx + d·dy) + 0.5` for a canvas point `d`
    /// from `center`. `None` when the picture is flat.
    pub fn inverse(&self) -> Option<[f32; 4]> {
        let [a, b, c, d] = self.linear();
        let det = a * d - b * c;
        let [w, h] = self.size;
        if det.abs() < 1e-6 || w.abs() < 1e-6 || h.abs() < 1e-6 {
            return None;
        }
        Some([d / det / w, -b / det / w, -c / det / h, a / det / h])
    }

    /// Picture `uv` under canvas point `p`, if the picture is not flat.
    pub fn to_picture(&self, p: [f32; 2]) -> Option<[f32; 2]> {
        if self.is_pinned() {
            return project(&self.uv_map()?, p);
        }
        self.unpinned_to_picture(p)
    }

    /// [`Self::to_picture`] without the corner pin.
    pub fn unpinned_to_picture(&self, p: [f32; 2]) -> Option<[f32; 2]> {
        let [a, b, c, d] = self.inverse()?;
        let (dx, dy) = (p[0] - self.center[0], p[1] - self.center[1]);
        Some([a * dx + b * dy + 0.5, c * dx + d * dy + 0.5])
    }

    /// The whole picture's corners on the canvas: top-left, top-right,
    /// bottom-right, bottom-left.
    pub fn corners(&self) -> [[f32; 2]; 4] {
        [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]].map(|uv| self.to_canvas(uv))
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

/// HTML generators in a headless browser (`flikk-html`).
pub trait HtmlService: Send + Sync {
    /// Load the document at `call.time` and return script and console errors
    /// (empty when clean).
    fn check(&self, call: &FxCall<'_>) -> ServiceResult<Vec<String>>;
    /// One frame with alpha, laid out at `call.canvas` and scaled to
    /// `width` × `height`.
    fn frame(&self, call: &FxCall<'_>, width: u32, height: u32) -> ServiceResult<Frame>;
    /// A service for one render, for engines that can make frames at once.
    fn crew(self: Arc<Self>) -> Option<Arc<dyn HtmlCrew>> {
        None
    }
}

/// An [`HtmlService`] for one render, which says what it will ask for next.
pub trait HtmlCrew: HtmlService {
    /// `call` will be asked for soon. Calls come in the order they show.
    fn ahead(&self, call: &FxCall<'_>, width: u32, height: u32);
}

/// Captions and titles drawn as pictures (`flikk-deliver`), in the caption
/// styles the service was made with.
pub trait LetteringService: Send + Sync {
    /// What `clip` (a caption on caption track `track`, or a title) shows at
    /// clip-relative time `t` in a `canvas`-sized frame, drawn `zoom` times
    /// its resting size so a scaled-up caption stays sharp. `None` when it
    /// shows nothing.
    fn letter(
        &self,
        track: &Track,
        clip: &Clip,
        t: Time,
        canvas: (u32, u32),
        zoom: f32,
    ) -> Option<Lettered>;
}

/// Lettering from [`LetteringService::letter`].
#[derive(Clone, Debug)]
pub struct Lettered {
    /// Tight around the lettering (box, stroke and shadow included), with
    /// alpha.
    pub frame: Frame,
    /// Where the picture rests before the clip's transform, in canvas
    /// pixels.
    pub center: [f32; 2],
    pub size: [f32; 2],
}

/// The engines the UI and agent host call. `None` until wired, or when the
/// machine lacks them (no GPU adapter, no Chromium); callers degrade quietly.
#[derive(Clone, Default)]
pub struct Services {
    pub media: Option<Arc<dyn MediaService>>,
    pub analysis: Option<Arc<dyn AnalysisService>>,
    pub fx: Option<Arc<dyn FxService>>,
    pub html: Option<Arc<dyn HtmlService>>,
    /// Draws captions and titles into the program. Renders leave it out:
    /// they burn lettering in with FFmpeg, or wire their own when FFmpeg
    /// cannot.
    pub lettering: Option<Arc<dyn LetteringService>>,
    /// Music and sound effects from the providers picked for them.
    pub generation: Option<Arc<dyn crate::cloud::GenerationService>>,
    /// Finds and follows the objects subject masks keep.
    pub subjects: Option<Arc<dyn SubjectService>>,
    /// Measures how the camera moved, for stabilized clips.
    pub stabilizer: Option<Arc<dyn crate::stabilize::Stabilizer>>,
    /// Makes frames between source frames for smooth slow motion.
    pub interpolation: Option<Arc<dyn InterpolationService>>,
    /// Depth maps for depth effects and masks.
    pub depth: Option<Arc<dyn crate::depth::DepthService>>,
}

impl std::fmt::Debug for Services {
    /// Which engines are wired.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Services")
            .field("media", &self.media.is_some())
            .field("analysis", &self.analysis.is_some())
            .field("fx", &self.fx.is_some())
            .field("html", &self.html.is_some())
            .field("lettering", &self.lettering.is_some())
            .field("generation", &self.generation.is_some())
            .field("subjects", &self.subjects.is_some())
            .field("stabilizer", &self.stabilizer.is_some())
            .field("interpolation", &self.interpolation.is_some())
            .field("depth", &self.depth.is_some())
            .finish()
    }
}
