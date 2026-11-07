//! Stabilization: a shaky handheld clip made smooth.
//!
//! Analysis measures how the camera moved from each frame of a media item
//! to the next ([`CameraPath`]: a similarity, translation, rotation and
//! scale, per frame), once per fingerprint ([`Stabilizer`], `flikk-analysis`).
//! A clip with [`Stabilization`] on adds the camera's moves up into a path
//! over the part of the source it shows, smooths it (a Gaussian whose width
//! the smoothness sets; [`StabilizeMode::Locked`] holds it still at its
//! mean), and moves each frame from where the camera was to where the
//! smooth path puts it ([`Plan`]). With [`StabilizeCrop::Auto`] the picture
//! is zoomed in by the least that hides the edges on every frame, at most
//! [`MAX_ZOOM`]; frames that would need more are corrected less, eased in
//! and out so the path stays smooth.
//!
//! The correction is a transform of the picture inside its own frame,
//! before its effects, masks and the clip's own transform
//! ([`crate::compose`]), so the viewer and composited renders draw it alike.

use crate::ids::JobId;
use crate::model::{Clip, ClipContent, Fingerprint, MediaItem, MediaKind, Transform};
use crate::services::{JobState, JobStatus, ServiceResult, Services};
use crate::time::{Time, TimeRange};
use serde::{Deserialize, Serialize};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

/// Marks [`CameraPath`]s; paths of another version are measured again.
pub const PATH_VERSION: u32 = 1;
/// Most the picture is zoomed in to hide the edges.
pub const MAX_ZOOM: f32 = 1.4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StabilizeMode {
    /// Follow the camera's intended move, without the shake.
    #[default]
    Smooth,
    /// Hold the frame still, as on a tripod, as far as the crop allows.
    Locked,
}

impl StabilizeMode {
    pub const ALL: [Self; 2] = [Self::Smooth, Self::Locked];

    pub fn label(self) -> &'static str {
        match self {
            Self::Smooth => "Smooth",
            Self::Locked => "Locked",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Smooth => "smooth",
            Self::Locked => "locked",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StabilizeCrop {
    /// Zoom in just enough that no edge shows.
    #[default]
    Auto,
    /// No zoom; the edges show where the frame moved.
    None,
}

impl StabilizeCrop {
    pub const ALL: [Self; 2] = [Self::Auto, Self::None];

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto zoom",
            Self::None => "None",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::None => "none",
        }
    }
}

/// A clip's stabilization. Off by default; turning it off keeps the
/// settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Stabilization {
    pub enabled: bool,
    /// 0..1: how long a move counts as shake, from about a tenth of a
    /// second to three seconds. Ignored when locked.
    pub smoothness: f32,
    pub mode: StabilizeMode,
    pub crop: StabilizeCrop,
}

impl Default for Stabilization {
    fn default() -> Self {
        Self {
            enabled: false,
            smoothness: 0.5,
            mode: StabilizeMode::Smooth,
            crop: StabilizeCrop::Auto,
        }
    }
}

impl Stabilization {
    /// Left out of project files.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Seconds of the Gaussian the camera path is smoothed with.
    pub fn sigma_secs(&self) -> f64 {
        0.12 * 25f64.powf(self.smoothness.clamp(0.0, 1.0) as f64)
    }
}

/// Whether `clip` is stabilized: on, and a video or image from the pool.
pub fn applies(clip: &Clip) -> bool {
    clip.stabilization.enabled && matches!(clip.content, ClipContent::Media { .. })
}

/// How the camera moved through a media item, frame by frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CameraPath {
    pub version: u32,
    /// Width over height of the frames measured.
    pub aspect: f32,
    /// Source time of each frame, seconds, rising.
    pub times: Vec<f64>,
    /// How each frame moved from the one before (the first: not at all):
    /// `[dx, dy, rotation, ln scale]`, so that a point `p` of the frame
    /// before, about the centre, is at `scale · R(rotation) · p + (dx, dy)`.
    /// Translations are fractions of the frame's width (y down), rotation
    /// radians clockwise.
    pub steps: Vec<[f32; 4]>,
}

/// One frame's place on a camera path: where frame 0's centre landed, and
/// how it turned and scaled.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Pose {
    x: f64,
    y: f64,
    angle: f64,
    ln_scale: f64,
}

impl Pose {
    /// The similarity `p ↦ a·p + b` as complex numbers (y down).
    fn a(&self) -> (f64, f64) {
        let s = self.ln_scale.exp();
        (s * self.angle.cos(), s * self.angle.sin())
    }
}

fn cmul(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0)
}

fn cdiv(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    let d = (b.0 * b.0 + b.1 * b.1).max(1e-12);
    ((a.0 * b.0 + a.1 * b.1) / d, (a.1 * b.0 - a.0 * b.1) / d)
}

/// How much a stretch of a path shakes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Shake {
    /// Root mean square of the jitter's translation, percent of the frame
    /// width.
    pub translation: f32,
    /// Root mean square of the jitter's rotation, degrees.
    pub rotation_deg: f32,
}

impl Shake {
    /// One number: the translation, plus the rotation as the move it makes
    /// at the frame's edge. Under 0.25 is steady, over 2 very shaky.
    pub fn score(&self) -> f32 {
        self.translation + self.rotation_deg * 0.87
    }

    pub fn label(&self) -> &'static str {
        match self.score() {
            s if s < 0.25 => "steady",
            s if s < 0.75 => "slightly shaky",
            s if s < 2.0 => "shaky",
            _ => "very shaky",
        }
    }
}

impl CameraPath {
    /// Frames `i0..=i1` covering source `range` (with the frame showing at
    /// its start); `None` with fewer than two.
    fn span(&self, range: TimeRange) -> Option<(usize, usize)> {
        let n = self.times.len().min(self.steps.len());
        if n < 2 {
            return None;
        }
        let (a, b) = (range.start.as_secs_f64(), range.end.as_secs_f64());
        let i0 = self.times[..n]
            .partition_point(|t| *t <= a)
            .saturating_sub(1);
        let i1 = self.times[..n].partition_point(|t| *t < b).max(i0 + 1);
        let i1 = i1.min(n - 1);
        (i1 > i0).then_some((i0, i1))
    }

    /// Frames per second, as measured.
    fn fps(&self) -> f64 {
        let n = self.times.len();
        if n < 2 {
            return 30.0;
        }
        let span = self.times[n - 1] - self.times[0];
        if span <= 0.0 {
            30.0
        } else {
            (n - 1) as f64 / span
        }
    }

    /// The camera's pose on frames `i0..=i1`, frame `i0` at rest.
    fn poses(&self, i0: usize, i1: usize) -> Vec<Pose> {
        let mut out = Vec::with_capacity(i1 - i0 + 1);
        let mut pose = Pose::default();
        out.push(pose);
        for step in &self.steps[i0 + 1..=i1] {
            let [dx, dy, da, ds] = step.map(|v| v as f64);
            let a = (ds.exp() * da.cos(), ds.exp() * da.sin());
            let (bx, by) = cmul(a, (pose.x, pose.y));
            pose = Pose {
                x: bx + dx,
                y: by + dy,
                angle: pose.angle + da,
                ln_scale: pose.ln_scale + ds,
            };
            out.push(pose);
        }
        out
    }

    /// How much source `range` shakes: what is left of the camera's path
    /// once its moves slower than a quarter of a second are taken out.
    pub fn shake(&self, range: TimeRange) -> Option<Shake> {
        let (i0, i1) = self.span(range)?;
        let poses = self.poses(i0, i1);
        let sigma = 0.25 * self.fps();
        let smooth = smooth_poses(&poses, sigma);
        let n = poses.len() as f64;
        let (mut t2, mut r2) = (0.0, 0.0);
        for (p, s) in poses.iter().zip(&smooth) {
            t2 += (p.x - s.x).powi(2) + (p.y - s.y).powi(2);
            r2 += (p.angle - s.angle).powi(2);
        }
        Some(Shake {
            translation: ((t2 / n).sqrt() * 100.0) as f32,
            rotation_deg: (r2 / n).sqrt().to_degrees() as f32,
        })
    }
}

/// Gaussian of `sigma` frames over each part of `poses` (see
/// [`gaussian`]).
fn smooth_poses(poses: &[Pose], sigma: f64) -> Vec<Pose> {
    let part = |f: fn(&Pose) -> f64| -> Vec<f64> {
        let values: Vec<f64> = poses.iter().map(f).collect();
        gaussian(&values, sigma)
    };
    let (x, y, angle, ln_scale) = (
        part(|p| p.x),
        part(|p| p.y),
        part(|p| p.angle),
        part(|p| p.ln_scale),
    );
    (0..poses.len())
        .map(|i| Pose {
            x: x[i],
            y: y[i],
            angle: angle[i],
            ln_scale: ln_scale[i],
        })
        .collect()
}

/// Where a line fitted to `values` passes their first: the end of a path,
/// without its last shake.
fn end_anchor(values: impl Iterator<Item = f64>) -> f64 {
    let v: Vec<f64> = values.collect();
    let n = v.len() as f64;
    if v.len() < 2 {
        return v.first().copied().unwrap_or(0.0);
    }
    let mx = (n - 1.0) / 2.0;
    let my = v.iter().sum::<f64>() / n;
    let (mut num, mut den) = (0.0, 0.0);
    for (i, y) in v.iter().enumerate() {
        num += (i as f64 - mx) * (y - my);
        den += (i as f64 - mx).powi(2);
    }
    my - num / den.max(1e-9) * mx
}

/// `values` smoothed by a Gaussian of `sigma` samples, cut at 3σ, with
/// each end reflected through a line fitted to the σ samples there, so a
/// pan carries on through it and its shake is smoothed too.
fn gaussian(values: &[f64], sigma: f64) -> Vec<f64> {
    let n = values.len();
    if n == 0 || sigma < 0.5 {
        return values.to_vec();
    }
    let fit = (sigma.ceil() as usize).clamp(2, n);
    let first = end_anchor(values[..fit].iter().copied());
    let last = end_anchor(values[n - fit..].iter().rev().copied());
    let radius = ((sigma * 3.0).ceil() as usize).min(4 * n);
    let kernel: Vec<f64> = (0..=radius)
        .map(|k| (-(k as f64).powi(2) / (2.0 * sigma * sigma)).exp())
        .collect();
    let last_value = last;
    let at = |i: isize| -> f64 {
        let last = n as isize - 1;
        if i < 0 {
            let j = (-i).min(last) as usize;
            2.0 * first - values[j]
        } else if i > last {
            let j = (2 * last - i).max(0) as usize;
            2.0 * last_value - values[j]
        } else {
            values[i as usize]
        }
    };
    (0..n as isize)
        .map(|i| {
            let mut sum = values[i as usize] * kernel[0];
            let mut weight = kernel[0];
            for (k, w) in kernel.iter().enumerate().skip(1) {
                let k = k as isize;
                sum += (at(i - k) + at(i + k)) * w;
                weight += 2.0 * w;
            }
            sum / weight
        })
        .collect()
}

/// The correction of one frame: the picture scaled and turned about its
/// centre, then moved.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Warp {
    pub scale: f32,
    /// Clockwise.
    pub rotation_deg: f32,
    /// Fractions of the picture's width and height.
    pub position: [f32; 2],
}

impl Default for Warp {
    fn default() -> Self {
        Self {
            scale: 1.0,
            rotation_deg: 0.0,
            position: [0.0; 2],
        }
    }
}

impl Warp {
    /// As a transform of a picture in a frame of its own size.
    pub fn transform(&self) -> Transform {
        Transform {
            position: self.position,
            scale: self.scale,
            rotation_deg: self.rotation_deg,
            ..Transform::default()
        }
    }

    pub fn is_identity(&self) -> bool {
        (self.scale - 1.0).abs() < 1e-5
            && self.rotation_deg.abs() < 1e-4
            && self.position.iter().all(|v| v.abs() < 1e-6)
    }
}

/// A similarity `p ↦ a·p + b` of the picture, about its centre, in frame
/// widths.
#[derive(Clone, Copy, Debug)]
struct Sim {
    a: (f64, f64),
    b: (f64, f64),
}

impl Sim {
    /// Taken `lambda` of the way from nothing.
    fn part(&self, lambda: f64) -> Sim {
        let (ln_s, angle) = (
            (self.a.0.hypot(self.a.1)).max(1e-12).ln(),
            self.a.1.atan2(self.a.0),
        );
        let s = (ln_s * lambda).exp();
        let angle = angle * lambda;
        Sim {
            a: (s * angle.cos(), s * angle.sin()),
            b: (self.b.0 * lambda, self.b.1 * lambda),
        }
    }

    /// Whether, zoomed `zoom` × about the centre, it covers the whole
    /// frame (half extents `half`).
    fn covers(&self, zoom: f64, half: (f64, f64)) -> bool {
        const SLACK: f64 = 1e-6;
        [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
            .into_iter()
            .all(|(sx, sy)| {
                let q = (sx * half.0 / zoom - self.b.0, sy * half.1 / zoom - self.b.1);
                let p = cdiv(q, self.a);
                p.0.abs() <= half.0 + SLACK && p.1.abs() <= half.1 + SLACK
            })
    }

    /// The least zoom that hides the edges; infinite past 4 ×.
    fn zoom_needed(&self, half: (f64, f64)) -> f64 {
        if self.covers(1.0, half) {
            return 1.0;
        }
        let (mut lo, mut hi) = (1.0, 4.0);
        if !self.covers(hi, half) {
            return f64::INFINITY;
        }
        for _ in 0..24 {
            let mid = 0.5 * (lo + hi);
            if self.covers(mid, half) {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        hi
    }
}

/// Each frame's correction over a clip's source range.
#[derive(Clone, Debug)]
pub struct Plan {
    times: Vec<f64>,
    warps: Vec<Warp>,
    /// The zoom in every warp (1 without crop).
    pub zoom: f32,
}

impl Plan {
    /// The correction of `path` with `settings` over source `range`;
    /// `None` when the path holds under two frames there.
    pub fn new(path: &CameraPath, settings: &Stabilization, range: TimeRange) -> Option<Plan> {
        let (i0, i1) = path.span(range)?;
        let poses = path.poses(i0, i1);
        let fps = path.fps();
        let target: Vec<Pose> = match settings.mode {
            StabilizeMode::Smooth => smooth_poses(&poses, settings.sigma_secs() * fps),
            StabilizeMode::Locked => {
                let n = poses.len() as f64;
                let mean = poses.iter().fold(Pose::default(), |m, p| Pose {
                    x: m.x + p.x / n,
                    y: m.y + p.y / n,
                    angle: m.angle + p.angle / n,
                    ln_scale: m.ln_scale + p.ln_scale / n,
                });
                vec![mean; poses.len()]
            }
        };
        // Each frame moves from where the camera was to where the target
        // path puts it: W = S ∘ C⁻¹.
        let mut sims: Vec<Sim> = poses
            .iter()
            .zip(&target)
            .map(|(c, s)| {
                let a = cdiv(s.a(), c.a());
                let ab = cmul(a, (c.x, c.y));
                Sim {
                    a,
                    b: (s.x - ab.0, s.y - ab.1),
                }
            })
            .collect();
        let aspect = path.aspect.max(0.05) as f64;
        let half = (0.5, 0.5 / aspect);
        let zoom = match settings.crop {
            StabilizeCrop::None => 1.0,
            StabilizeCrop::Auto => {
                let max = MAX_ZOOM as f64;
                // How much of each frame's correction fits under the most
                // zoom, eased so the path stays smooth: a running minimum,
                // then a Gaussian no wider, never above the minimum.
                let fits: Vec<f64> = sims
                    .iter()
                    .map(|w| {
                        if w.zoom_needed(half) <= max {
                            return 1.0;
                        }
                        let (mut lo, mut hi) = (0.0, 1.0);
                        for _ in 0..16 {
                            let mid = 0.5 * (lo + hi);
                            if w.part(mid).zoom_needed(half) <= max {
                                lo = mid;
                            } else {
                                hi = mid;
                            }
                        }
                        lo
                    })
                    .collect();
                if fits.iter().any(|f| *f < 1.0) {
                    let reach = ((0.5 * fps).ceil() as usize).max(1);
                    let lowest: Vec<f64> = (0..fits.len())
                        .map(|i| {
                            let a = i.saturating_sub(reach);
                            let b = (i + reach).min(fits.len() - 1);
                            fits[a..=b].iter().cloned().fold(1.0, f64::min)
                        })
                        .collect();
                    let eased = gaussian_within(&lowest, reach);
                    for (w, l) in sims.iter_mut().zip(eased) {
                        *w = w.part(l.clamp(0.0, 1.0));
                    }
                }
                sims.iter()
                    .map(|w| w.zoom_needed(half))
                    .fold(1.0, f64::max)
                    .min(max)
            }
        };
        let warps = sims
            .iter()
            .map(|w| {
                let a = (w.a.0 * zoom, w.a.1 * zoom);
                Warp {
                    scale: a.0.hypot(a.1) as f32,
                    rotation_deg: a.1.atan2(a.0).to_degrees() as f32,
                    position: [(w.b.0 * zoom) as f32, (w.b.1 * zoom * aspect) as f32],
                }
            })
            .collect();
        Some(Plan {
            times: path.times[i0..=i1].to_vec(),
            warps,
            zoom: zoom as f32,
        })
    }

    /// The correction of the frame showing at source time `t` (the first
    /// or last outside the range).
    pub fn at(&self, t: Time) -> Warp {
        let t = t.as_secs_f64();
        let i = self.times.partition_point(|x| *x <= t + 1e-4);
        self.warps[i.saturating_sub(1).min(self.warps.len() - 1)]
    }
}

impl Plan {
    /// The correction at source time `t` between two frames, for a
    /// moment made between them (a slowed clip that blends or flows):
    /// theirs, taken in proportion.
    pub fn between(&self, t: Time) -> Warp {
        let secs = t.as_secs_f64();
        let i = self.times.partition_point(|x| *x <= secs + 1e-4);
        if i == 0 || i >= self.times.len() {
            return self.at(t);
        }
        let (a, b) = (self.warps[i - 1], self.warps[i]);
        let (ta, tb) = (self.times[i - 1], self.times[i]);
        let f = if tb > ta {
            ((secs - ta) / (tb - ta)).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        let mix = |x: f32, y: f32| x + (y - x) * f;
        Warp {
            scale: mix(a.scale, b.scale),
            rotation_deg: mix(a.rotation_deg, b.rotation_deg),
            position: [
                mix(a.position[0], b.position[0]),
                mix(a.position[1], b.position[1]),
            ],
        }
    }
}

/// `values` smoothed by a Gaussian cut at `reach` samples (σ = reach / 2),
/// the ends held.
fn gaussian_within(values: &[f64], reach: usize) -> Vec<f64> {
    let sigma = (reach as f64 / 2.0).max(0.5);
    let n = values.len() as isize;
    (0..n)
        .map(|i| {
            let (mut sum, mut weight) = (0.0, 0.0);
            for k in -(reach as isize)..=reach as isize {
                let j = (i + k).clamp(0, n - 1) as usize;
                let w = (-(k as f64).powi(2) / (2.0 * sigma * sigma)).exp();
                sum += values[j] * w;
                weight += w;
            }
            sum / weight
        })
        .collect()
}

/// Where measuring an item's camera path is.
#[derive(Clone, Debug, PartialEq)]
pub enum MotionStatus {
    /// Not measured, nor being measured.
    Missing,
    Queued,
    Measuring(f32),
    Ready,
    Failed(String),
}

/// Measures how the camera moved through media items (`flikk-analysis`),
/// keeping each item's path by fingerprint.
pub trait Stabilizer: Send + Sync {
    /// The item's camera path, once measured (from memory or the cache).
    fn path(&self, item: &MediaItem) -> Option<Arc<CameraPath>>;
    /// Queue measuring the item's path, unless it is measured, queued or
    /// failed already.
    fn measure(&self, item: &MediaItem) -> Option<JobId>;
    /// Measure the item's path now, waiting for it (renders, the agent).
    /// `progress` gets 0..1.
    fn complete(
        &self,
        item: &MediaItem,
        progress: &dyn Fn(f32),
        cancel: &AtomicBool,
    ) -> ServiceResult<Arc<CameraPath>>;
    /// Stop measuring `item`'s path in the background, queued or running,
    /// kept as not measured, so asking again starts afresh. For media that
    /// left the project; [`Stabilizer::complete`] is not stopped by it.
    fn cancel(&self, _item: &MediaItem) {}
    /// Measuring jobs, as the job queue lists them.
    fn jobs(&self) -> Vec<JobStatus>;
    /// Counts paths measured, so views know to draw again.
    fn version(&self) -> u64;

    /// Where the item's path is.
    fn status(&self, item: &MediaItem) -> MotionStatus {
        if self.path(item).is_some() {
            return MotionStatus::Ready;
        }
        let job = self
            .jobs()
            .into_iter()
            .rev()
            .find(|j| j.media == Some(item.id));
        match job.map(|j| j.state) {
            None | Some(JobState::Done) => MotionStatus::Missing,
            Some(JobState::Queued) => MotionStatus::Queued,
            Some(JobState::Running { fraction }) => MotionStatus::Measuring(fraction),
            Some(JobState::Failed { message }) => MotionStatus::Failed(message),
        }
    }
}

/// The media item under `clip` and the source range it shows, when the
/// clip is stabilized and the item can be measured (a video with a
/// fingerprint).
pub fn source<'a>(
    project: &'a crate::model::Project,
    clip: &Clip,
) -> Option<(&'a MediaItem, TimeRange)> {
    if !applies(clip) {
        return None;
    }
    let ClipContent::Media { media, .. } = &clip.content else {
        return None;
    };
    let item = project.media(*media)?;
    if item.kind != MediaKind::Video || item.fingerprint.is_none() {
        return None;
    }
    let (a, b) = (clip.to_source(clip.start)?, clip.to_source(clip.end())?);
    Some((item, TimeRange::new(a.min(b), a.max(b))))
}

/// Plans made lately, newest last, with the path each was made from (so
/// its address names it while it is kept).
type Kept = (PlanKey, Arc<CameraPath>, Arc<Plan>);
type PlanKey = (
    Fingerprint,
    usize,
    u32,
    StabilizeMode,
    StabilizeCrop,
    TimeRange,
);
static PLANS: Mutex<Vec<Kept>> = Mutex::new(Vec::new());
const PLANS_KEPT: usize = 24;

/// The plan for `clip` over source `range` on `item`'s path, made once
/// and kept for the frames after.
pub fn plan(
    stabilizer: &dyn Stabilizer,
    clip: &Clip,
    item: &MediaItem,
    range: TimeRange,
) -> Option<Arc<Plan>> {
    let Some(path) = stabilizer.path(item) else {
        stabilizer.measure(item);
        return None;
    };
    let s = &clip.stabilization;
    let key: PlanKey = (
        item.fingerprint.clone()?,
        Arc::as_ptr(&path) as usize,
        s.smoothness.to_bits(),
        s.mode,
        s.crop,
        range,
    );
    let mut kept = PLANS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(i) = kept.iter().position(|(k, _, _)| *k == key) {
        let entry = kept.remove(i);
        let plan = entry.2.clone();
        kept.push(entry);
        return Some(plan);
    }
    drop(kept);
    let plan = Arc::new(Plan::new(&path, s, range)?);
    let mut kept = PLANS.lock().unwrap_or_else(|e| e.into_inner());
    kept.push((key, path, plan.clone()));
    if kept.len() > PLANS_KEPT {
        kept.remove(0);
    }
    Some(plan)
}

/// The correction `clip` draws its picture with at timeline time `t`
/// (with `between`, a moment between two source frames: see
/// [`Plan::between`]); `None` when it is not stabilized or its path is
/// not measured yet (measuring it then starts). Source times follow the
/// clip's speed and ramp ([`Clip::to_source`]).
pub fn correction(
    project: &crate::model::Project,
    services: &Services,
    clip: &Clip,
    t: Time,
    between: bool,
) -> Option<Warp> {
    let stabilizer = services.stabilizer.as_deref()?;
    let (item, range) = source(project, clip)?;
    let plan = plan(stabilizer, clip, item, range)?;
    let source = clip.to_source(t)?;
    let warp = if between {
        plan.between(source)
    } else {
        plan.at(source)
    };
    (!warp.is_identity()).then_some(warp)
}
