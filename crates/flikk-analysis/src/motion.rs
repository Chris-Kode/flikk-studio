//! Camera motion for stabilization: [`LocalStabilizer`] implements
//! `flikk_core::stabilize::Stabilizer` in Rust alone, without a model.
//!
//! Frames are read once, in order, through the media engine (the proxy
//! when there is one) at most [`WIDTH`] pixels wide, and turned grey. Each
//! frame is compared with the one before: first the whole picture, a
//! quarter the size, for how far it moved at all; then a grid of blocks,
//! each found again in the new frame by its least absolute difference,
//! searched at half size around that move and refined at full size to a
//! fraction of a pixel. Blocks without texture in both directions are left
//! out. A move and turn are fitted to the blocks' moves by RANSAC and refined on the blocks that agree with it, so a subject that
//! moves on its own does not move the camera; with too few agreeing, the
//! median move stands in.
//!
//! Paths are kept by fingerprint in memory and under
//! `paths::cache()/analysis/<fingerprint>.motion`, beside the analysis
//! results: the cache's Clear in Settings → Storage leaves them.
//!
//! An item is measured by one thread at a time: one that wants it while
//! another measures it waits for that and takes its path, so a render
//! does not measure again what the background is measuring. One that
//! failed is not queued again for [`RETRY_AFTER`]; renders measure it
//! again all the same. A background job stops, kept as not measured,
//! when its item is asked for under another fingerprint (the file
//! changed), is cancelled (it left the project) or the stabilizer is
//! dropped (the window closed).

use flikk_core::ids::{JobId, MediaId};
use flikk_core::jobs::JobQueue;
use flikk_core::model::{Fingerprint, MediaItem, MediaKind};
use flikk_core::paths;
use flikk_core::services::JobState;
use flikk_core::services::{Frame, JobStatus, MediaService, ServiceError, ServiceResult};
use flikk_core::stabilize::{CameraPath, PATH_VERSION, Stabilizer};
use flikk_core::time::Time;
use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Widest frame measured.
const WIDTH: u32 = 320;
/// Side of a block, at full size.
const BLOCK: usize = 16;
/// Blocks across and down.
const GRID: (usize, usize) = (10, 6);
/// How far a block is searched at half size, either way (twice that at
/// full size), around the whole picture's move.
const SEARCH: i32 = 8;
/// How far the whole picture is searched at quarter size, either way.
const GLOBAL_SEARCH: i32 = 8;
/// Pixels (full size) a block may be off the fitted motion and agree.
const INLIER: f64 = 1.0;
const RANSAC_ROUNDS: usize = 200;
/// Most a frame turns from the one before, radians (about 3°).
const MAX_TURN: f64 = 0.05;
/// Least texture (the smaller eigenvalue of a block's structure tensor,
/// per pixel) a block needs to be measured.
const TEXTURE: f64 = 30.0;
/// How long an item that failed is left before it is queued again.
const RETRY_AFTER: Duration = Duration::from_secs(30);
/// How often a thread waiting for another's measuring looks at `cancel`.
const WAIT_STEP: Duration = Duration::from_millis(100);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What a panic measuring says, with its message when it carries one.
fn stopped(payload: &(dyn Any + Send)) -> String {
    let detail = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str));
    match detail {
        Some(detail) => format!("measuring the camera path stopped unexpectedly: {detail}"),
        None => "measuring the camera path stopped unexpectedly".into(),
    }
}

/// The [`Stabilizer`] that measures on this machine.
pub struct LocalStabilizer {
    shared: Arc<Shared>,
    queue: JobQueue<Fingerprint>,
}

struct Shared {
    media: Arc<dyn MediaService>,
    paths: Mutex<HashMap<Fingerprint, Arc<CameraPath>>>,
    /// Looked for on disk and not there.
    absent: Mutex<HashSet<Fingerprint>>,
    /// Items being measured now, and their end announced.
    measuring: Mutex<HashSet<Fingerprint>>,
    measured: Condvar,
    /// Items that failed: why, and when.
    failed: Mutex<HashMap<Fingerprint, (String, Instant)>>,
    /// The fingerprint each media item was last asked for under.
    wanted: Mutex<HashMap<MediaId, Fingerprint>>,
    /// The stabilizer was dropped: background jobs stop.
    closed: AtomicBool,
    version: AtomicU64,
}

impl LocalStabilizer {
    pub fn new(media: Arc<dyn MediaService>) -> Self {
        Self {
            shared: Arc::new(Shared {
                media,
                paths: Mutex::default(),
                absent: Mutex::default(),
                measuring: Mutex::default(),
                measured: Condvar::new(),
                failed: Mutex::default(),
                wanted: Mutex::default(),
                closed: AtomicBool::new(false),
                version: AtomicU64::new(0),
            }),
            queue: JobQueue::new("flikk-motion", 1),
        }
    }
}

impl Drop for LocalStabilizer {
    fn drop(&mut self) {
        // The jobs hold `shared`, not this: tell them nobody is left.
        self.shared.closed.store(true, Ordering::Relaxed);
        self.shared.measured.notify_all();
    }
}

impl Shared {
    fn path(&self, fp: &Fingerprint) -> Option<Arc<CameraPath>> {
        if let Some(p) = lock(&self.paths).get(fp) {
            return Some(p.clone());
        }
        if lock(&self.absent).contains(fp) {
            return None;
        }
        match load(fp) {
            Some(path) => {
                let path = Arc::new(path);
                lock(&self.paths).insert(fp.clone(), path.clone());
                Some(path)
            }
            None => {
                lock(&self.absent).insert(fp.clone());
                None
            }
        }
    }

    /// Whether `fp` failed less than [`RETRY_AFTER`] ago, and why.
    fn failed_lately(&self, fp: &Fingerprint) -> Option<String> {
        lock(&self.failed)
            .get(fp)
            .filter(|(_, at)| at.elapsed() < RETRY_AFTER)
            .map(|(why, _)| why.clone())
    }

    /// Measure `item`, or take what another thread measuring it found,
    /// calling `waiting` while it waits for that. When it failed lately,
    /// measures it again if `again`, and fails with that error otherwise.
    /// Stops when `cancel` says so; that is not kept as a failure.
    fn measure(
        &self,
        item: &MediaItem,
        fp: &Fingerprint,
        progress: &dyn Fn(f32),
        cancel: &dyn Fn() -> bool,
        again: bool,
        waiting: &dyn Fn(),
    ) -> ServiceResult<Arc<CameraPath>> {
        {
            let mut measuring = lock(&self.measuring);
            loop {
                if let Some(path) = self.path(fp) {
                    return Ok(path);
                }
                if !again && let Some(why) = self.failed_lately(fp) {
                    return Err(ServiceError(why));
                }
                if measuring.insert(fp.clone()) {
                    break;
                }
                if cancel() {
                    return Err(ServiceError("cancelled".into()));
                }
                waiting();
                measuring = self
                    .measured
                    .wait_timeout(measuring, WAIT_STEP)
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
        }
        let _claim = Measuring(self, fp);
        if cancel() {
            return Err(ServiceError("cancelled".into()));
        }
        let result = catch_unwind(AssertUnwindSafe(|| {
            measure(self.media.as_ref(), item, progress, cancel)
        }))
        .unwrap_or_else(|payload| Err(ServiceError(stopped(payload.as_ref()))));
        match result {
            Ok(path) => {
                if let Err(e) = store(fp, &path) {
                    eprintln!("flikk-analysis: could not keep the camera path: {e}");
                }
                let path = Arc::new(path);
                lock(&self.paths).insert(fp.clone(), path.clone());
                lock(&self.absent).remove(fp);
                lock(&self.failed).remove(fp);
                self.version.fetch_add(1, Ordering::Relaxed);
                Ok(path)
            }
            Err(e) => {
                if !cancel() {
                    lock(&self.failed).insert(fp.clone(), (e.0.clone(), Instant::now()));
                }
                Err(e)
            }
        }
    }
}

/// An item claimed in [`Shared::measuring`], let go when dropped.
struct Measuring<'a>(&'a Shared, &'a Fingerprint);

impl Drop for Measuring<'_> {
    fn drop(&mut self) {
        lock(&self.0.measuring).remove(self.1);
        self.0.measured.notify_all();
    }
}

impl Stabilizer for LocalStabilizer {
    fn path(&self, item: &MediaItem) -> Option<Arc<CameraPath>> {
        self.shared.path(item.fingerprint.as_ref()?)
    }

    fn measure(&self, item: &MediaItem) -> Option<JobId> {
        let fp = item.fingerprint.clone()?;
        if item.kind != MediaKind::Video {
            return None;
        }
        {
            // The item's file changed: what it was queued under is not
            // wanted, unless another item has that fingerprint too.
            let mut wanted = lock(&self.shared.wanted);
            if let Some(old) = wanted.insert(item.id, fp.clone())
                && old != fp
                && !wanted.values().any(|f| *f == old)
            {
                self.queue.cancel(&old);
            }
        }
        if self.shared.path(&fp).is_some() || self.shared.failed_lately(&fp).is_some() {
            return None;
        }
        let shared = self.shared.clone();
        let owned = item.clone();
        let name = item
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let key = fp.clone();
        Some(self.queue.push(
            key,
            format!("Stabilize {name}"),
            Some(item.id),
            move |job| {
                let cancel = || job.cancelled() || shared.closed.load(Ordering::Relaxed);
                shared
                    .measure(&owned, &fp, &|p| job.set(p), &cancel, false, &|| {})
                    .map(|_| ())
                    .map_err(|e| e.0)
            },
        ))
    }

    fn complete(
        &self,
        item: &MediaItem,
        progress: &dyn Fn(f32),
        cancel: &AtomicBool,
    ) -> ServiceResult<Arc<CameraPath>> {
        let fp = item
            .fingerprint
            .clone()
            .ok_or_else(|| ServiceError("the media has no fingerprint yet".into()))?;
        if let Some(path) = self.shared.path(&fp) {
            return Ok(path);
        }
        // While the queue's job measures it, report how far that got.
        let waiting = || {
            let job = self
                .queue
                .jobs()
                .into_iter()
                .rev()
                .find(|j| j.media == Some(item.id));
            if let Some(JobState::Running { fraction }) = job.map(|j| j.state) {
                progress(fraction);
            }
        };
        let cancelled = || cancel.load(Ordering::Relaxed);
        self.shared
            .measure(item, &fp, progress, &cancelled, true, &waiting)
    }

    fn cancel(&self, item: &MediaItem) {
        let mut wanted = lock(&self.shared.wanted);
        // What it was queued under, and what it is under now.
        let queued = wanted.remove(&item.id);
        let fps = queued.into_iter().chain(item.fingerprint.clone());
        for fp in fps {
            // Unless another item has that fingerprint too.
            if !wanted.values().any(|f| *f == fp) {
                self.queue.cancel(&fp);
            }
        }
    }

    fn jobs(&self) -> Vec<JobStatus> {
        self.queue.jobs()
    }

    fn version(&self) -> u64 {
        self.shared.version.load(Ordering::Relaxed)
    }
}

// ------------------------------------------------------------------ cache

fn file(fp: &Fingerprint) -> PathBuf {
    let name: String =
        fp.0.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
    paths::cache()
        .join("analysis")
        .join(format!("{name}.motion"))
}

fn load(fp: &Fingerprint) -> Option<CameraPath> {
    let path: CameraPath = serde_json::from_slice(&std::fs::read(file(fp)).ok()?).ok()?;
    (path.version == PATH_VERSION).then_some(path)
}

fn store(fp: &Fingerprint, path: &CameraPath) -> anyhow::Result<()> {
    let dest = file(fp);
    let dir = dest.parent().expect("cache file has a parent");
    std::fs::create_dir_all(dir)?;
    let tmp = dest.with_extension(format!("motion.{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec(path)?)?;
    std::fs::rename(&tmp, &dest).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(())
}

// ------------------------------------------------------------------ measuring

/// The camera path of `item`, read frame by frame.
fn measure(
    media: &dyn MediaService,
    item: &MediaItem,
    progress: &dyn Fn(f32),
    cancel: &dyn Fn() -> bool,
) -> ServiceResult<CameraPath> {
    let duration = item
        .info
        .as_ref()
        .map(|i| i.duration.as_secs_f64())
        .unwrap_or(0.0);
    let mut stream = media.playback(item, Time::ZERO, WIDTH)?;
    let mut times = Vec::new();
    let mut steps = Vec::new();
    let mut previous: Option<Pyramid> = None;
    let mut aspect = 16.0 / 9.0;
    while let Some((t, frame)) = stream.next_frame()? {
        if cancel() {
            return Err(ServiceError("cancelled".into()));
        }
        aspect = frame.width.max(1) as f32 / frame.height.max(1) as f32;
        let now = Pyramid::new(&frame);
        let step = match &previous {
            Some(before) if before.full.w == now.full.w && before.full.h == now.full.h => {
                estimate(before, &now)
            }
            _ => [0.0; 4],
        };
        times.push(t.as_secs_f64());
        steps.push(step);
        previous = Some(now);
        if duration > 0.0 && times.len() % 8 == 0 {
            progress((t.as_secs_f64() / duration).clamp(0.0, 1.0) as f32);
        }
    }
    if times.is_empty() {
        return Err(ServiceError("no frames could be read".into()));
    }
    progress(1.0);
    Ok(CameraPath {
        version: PATH_VERSION,
        aspect,
        times,
        steps,
    })
}

/// A grey picture.
struct Grey {
    w: usize,
    h: usize,
    px: Vec<u8>,
}

impl Grey {
    fn of(frame: &Frame) -> Grey {
        let (w, h) = (frame.width as usize, frame.height as usize);
        let px = frame
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .take(w * h)
            .map(|p| ((p[0] as u32 * 77 + p[1] as u32 * 150 + p[2] as u32 * 29) >> 8) as u8)
            .collect();
        Grey { w, h, px }
    }

    /// Half the size, each pixel the mean of four.
    fn half(&self) -> Grey {
        let (w, h) = ((self.w / 2).max(1), (self.h / 2).max(1));
        let mut px = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let at = |dx: usize, dy: usize| {
                    let (sx, sy) = ((2 * x + dx).min(self.w - 1), (2 * y + dy).min(self.h - 1));
                    self.px[sy * self.w + sx] as u32
                };
                px.push(((at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1) + 2) / 4) as u8);
            }
        }
        Grey { w, h, px }
    }

    fn at(&self, x: i32, y: i32) -> u8 {
        let x = x.clamp(0, self.w as i32 - 1) as usize;
        let y = y.clamp(0, self.h as i32 - 1) as usize;
        self.px[y * self.w + x]
    }

    /// Sum of absolute differences between the `size` block at `(x, y)`
    /// here and at `(x + dx, y + dy)` in `other`, every `step` pixels.
    #[allow(clippy::too_many_arguments)]
    fn sad(
        &self,
        other: &Grey,
        (x, y): (i32, i32),
        size: i32,
        (dx, dy): (i32, i32),
        step: i32,
    ) -> u32 {
        let mut sum = 0u32;
        let mut j = 0;
        while j < size {
            let mut i = 0;
            while i < size {
                let a = self.at(x + i, y + j) as i32;
                let b = other.at(x + i + dx, y + j + dy) as i32;
                sum += (a - b).unsigned_abs();
                i += step;
            }
            j += step;
        }
        sum
    }

    /// The smaller eigenvalue of the structure tensor of the `size` block
    /// at `(x, y)`, per pixel: high only where there is texture both ways.
    fn texture(&self, (x, y): (i32, i32), size: i32) -> f64 {
        let (mut xx, mut yy, mut xy) = (0.0, 0.0, 0.0);
        for j in 0..size {
            for i in 0..size {
                let gx =
                    (self.at(x + i + 1, y + j) as f64 - self.at(x + i - 1, y + j) as f64) / 2.0;
                let gy =
                    (self.at(x + i, y + j + 1) as f64 - self.at(x + i, y + j - 1) as f64) / 2.0;
                xx += gx * gx;
                yy += gy * gy;
                xy += gx * gy;
            }
        }
        let n = (size * size) as f64;
        let (a, b, c) = (xx / n, yy / n, xy / n);
        let mean = (a + b) / 2.0;
        mean - (((a - b) / 2.0).powi(2) + c * c).sqrt()
    }
}

/// A frame at full, half and quarter size.
struct Pyramid {
    full: Grey,
    half: Grey,
    quarter: Grey,
}

impl Pyramid {
    fn new(frame: &Frame) -> Pyramid {
        let full = Grey::of(frame);
        let half = full.half();
        let quarter = half.half();
        Pyramid {
            full,
            half,
            quarter,
        }
    }
}

/// The offset in `-r..=r` (both ways, around `around`) where `f` is least,
/// and that least value.
fn best(around: (i32, i32), r: i32, mut f: impl FnMut(i32, i32) -> u32) -> ((i32, i32), u32) {
    let mut found = (around, u32::MAX);
    for dy in -r..=r {
        for dx in -r..=r {
            let d = (around.0 + dx, around.1 + dy);
            let v = f(d.0, d.1);
            if v < found.1 {
                found = (d, v);
            }
        }
    }
    found
}

/// Where between three samples their parabola is least, -0.5..0.5.
fn vertex(left: f64, mid: f64, right: f64) -> f64 {
    let d = left - 2.0 * mid + right;
    if d <= 1e-9 {
        0.0
    } else {
        (0.5 * (left - right) / d).clamp(-0.5, 0.5)
    }
}

/// How `now` moved from `before`: `[dx, dy, rotation, ln scale]` (see
/// [`CameraPath::steps`]).
fn estimate(before: &Pyramid, now: &Pyramid) -> [f32; 4] {
    let (w, h) = (before.full.w as i32, before.full.h as i32);
    // The whole picture's move, at quarter size, on its middle.
    let (qw, qh) = (before.quarter.w as i32, before.quarter.h as i32);
    let margin = GLOBAL_SEARCH + 1;
    let inner = (qw - 2 * margin).min(qh - 2 * margin);
    let global = if inner >= 8 {
        let origin = ((qw - inner) / 2, (qh - inner) / 2);
        let (d, _) = best((0, 0), GLOBAL_SEARCH, |dx, dy| {
            before.quarter.sad(&now.quarter, origin, inner, (dx, dy), 1)
        });
        (d.0 * 2, d.1 * 2)
    } else {
        (0, 0)
    };

    // Each block: half size around the global move, then full size.
    let b = BLOCK as i32;
    let reach = 2 * SEARCH + 2 * global.0.abs().max(global.1.abs()) + 4;
    let x0 = reach.min(w / 4);
    let x1 = (w - b - reach).max(x0);
    let y0 = reach.min(h / 4);
    let y1 = (h - b - reach).max(y0);
    let mut pairs: Vec<Pair> = Vec::new();
    for gy in 0..GRID.1 {
        for gx in 0..GRID.0 {
            let x = x0 + (x1 - x0) * gx as i32 / (GRID.0 as i32 - 1).max(1);
            let y = y0 + (y1 - y0) * gy as i32 / (GRID.1 as i32 - 1).max(1);
            if before.full.texture((x, y), b) < TEXTURE {
                continue;
            }
            let (half_d, _) = best(global, SEARCH, |dx, dy| {
                before
                    .half
                    .sad(&now.half, (x / 2, y / 2), b / 2, (dx, dy), 1)
            });
            let around = (half_d.0 * 2, half_d.1 * 2);
            let mut cost = |dx: i32, dy: i32| before.full.sad(&now.full, (x, y), b, (dx, dy), 1);
            let (d, least) = best(around, 2, &mut cost);
            let sx = vertex(
                cost(d.0 - 1, d.1) as f64,
                least as f64,
                cost(d.0 + 1, d.1) as f64,
            );
            let sy = vertex(
                cost(d.0, d.1 - 1) as f64,
                least as f64,
                cost(d.0, d.1 + 1) as f64,
            );
            let p = (
                (x + b / 2) as f64 - w as f64 / 2.0,
                (y + b / 2) as f64 - h as f64 / 2.0,
            );
            let q = (p.0 + d.0 as f64 + sx, p.1 + d.1 as f64 + sy);
            pairs.push((p, q));
        }
    }
    let sim = fit(&pairs);
    let (a, bb) = sim;
    let scale = a.0.hypot(a.1).max(1e-6);
    [
        (bb.0 / w as f64) as f32,
        (bb.1 / w as f64) as f32,
        a.1.atan2(a.0) as f32,
        scale.ln() as f32,
    ]
}

type Similarity = ((f64, f64), (f64, f64));
/// A block's centre in the frame before and in this one, about the
/// centre, in pixels.
type Pair = ((f64, f64), (f64, f64));

/// The move and turn `q = a·p + b` (`|a| = 1`) most `pairs` agree with:
/// RANSAC on pairs of pairs, refined by least squares on those that agree;
/// the median move when too few do. Shake hardly zooms from one frame to
/// the next, and a fitted zoom was mostly noise, so none is.
fn fit(pairs: &[Pair]) -> Similarity {
    let identity = ((1.0, 0.0), (0.0, 0.0));
    if pairs.len() < 3 {
        return match pairs.first() {
            Some(_) => median_move(pairs),
            None => identity,
        };
    }
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15 ^ pairs.len() as u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let n = pairs.len();
    let mut best_inliers: Vec<usize> = Vec::new();
    for _ in 0..RANSAC_ROUNDS {
        let i = (next() % n as u64) as usize;
        let j = (next() % n as u64) as usize;
        if i == j {
            continue;
        }
        let Some(model) = two_point(pairs[i], pairs[j]) else {
            continue;
        };
        let inliers = agreeing(pairs, model);
        if inliers.len() > best_inliers.len() {
            best_inliers = inliers;
        }
    }
    let enough = best_inliers.len() >= 4 && best_inliers.len() * 3 >= n;
    if !enough {
        return median_move(pairs);
    }
    let chosen: Vec<_> = best_inliers.iter().map(|&i| pairs[i]).collect();
    let refined = least_squares_rigid(&chosen);
    // Once more, on what agrees with the refined model.
    let again: Vec<_> = agreeing(pairs, refined)
        .into_iter()
        .map(|i| pairs[i])
        .collect();
    let model = if again.len() >= chosen.len() {
        least_squares_rigid(&again)
    } else {
        refined
    };
    if model.0.1.atan2(model.0.0).abs() > MAX_TURN {
        return median_move(pairs);
    }
    model
}

/// [`least_squares`] without scale.
fn least_squares_rigid(pairs: &[Pair]) -> Similarity {
    let (a, _) = least_squares(pairs);
    let len = a.0.hypot(a.1).max(1e-9);
    let a = (a.0 / len, a.1 / len);
    let n = pairs.len() as f64;
    let (mut pm, mut qm) = ((0.0, 0.0), (0.0, 0.0));
    for (p, q) in pairs {
        pm = (pm.0 + p.0 / n, pm.1 + p.1 / n);
        qm = (qm.0 + q.0 / n, qm.1 + q.1 / n);
    }
    let b = (
        qm.0 - (a.0 * pm.0 - a.1 * pm.1),
        qm.1 - (a.0 * pm.1 + a.1 * pm.0),
    );
    (a, b)
}

fn agreeing(pairs: &[Pair], (a, b): Similarity) -> Vec<usize> {
    pairs
        .iter()
        .enumerate()
        .filter(|(_, (p, q))| {
            let m = (a.0 * p.0 - a.1 * p.1 + b.0, a.0 * p.1 + a.1 * p.0 + b.1);
            (m.0 - q.0).hypot(m.1 - q.1) <= INLIER
        })
        .map(|(i, _)| i)
        .collect()
}

fn two_point((p1, q1): Pair, (p2, q2): Pair) -> Option<Similarity> {
    let dp = (p2.0 - p1.0, p2.1 - p1.1);
    let dq = (q2.0 - q1.0, q2.1 - q1.1);
    let d = dp.0 * dp.0 + dp.1 * dp.1;
    if d < 16.0 {
        return None;
    }
    // Rigid: the turn between the two, no scale.
    let a = (dq.0 * dp.0 + dq.1 * dp.1, dq.1 * dp.0 - dq.0 * dp.1);
    let len = a.0.hypot(a.1);
    if len < 1e-9 {
        return None;
    }
    let a = (a.0 / len, a.1 / len);
    let b = (
        q1.0 - (a.0 * p1.0 - a.1 * p1.1),
        q1.1 - (a.0 * p1.1 + a.1 * p1.0),
    );
    Some((a, b))
}

fn least_squares(pairs: &[Pair]) -> Similarity {
    let n = pairs.len() as f64;
    let (mut pm, mut qm) = ((0.0, 0.0), (0.0, 0.0));
    for (p, q) in pairs {
        pm = (pm.0 + p.0 / n, pm.1 + p.1 / n);
        qm = (qm.0 + q.0 / n, qm.1 + q.1 / n);
    }
    let (mut num, mut den) = ((0.0, 0.0), 0.0);
    for (p, q) in pairs {
        let (pc, qc) = ((p.0 - pm.0, p.1 - pm.1), (q.0 - qm.0, q.1 - qm.1));
        // q · conj(p)
        num = (
            num.0 + qc.0 * pc.0 + qc.1 * pc.1,
            num.1 + qc.1 * pc.0 - qc.0 * pc.1,
        );
        den += pc.0 * pc.0 + pc.1 * pc.1;
    }
    let a = if den > 1e-9 {
        (num.0 / den, num.1 / den)
    } else {
        (1.0, 0.0)
    };
    let b = (
        qm.0 - (a.0 * pm.0 - a.1 * pm.1),
        qm.1 - (a.0 * pm.1 + a.1 * pm.0),
    );
    (a, b)
}

/// The median of the blocks' moves, as a move alone.
fn median_move(pairs: &[Pair]) -> Similarity {
    let median = |mut v: Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v.get(v.len() / 2).copied().unwrap_or(0.0)
    };
    let dx = median(pairs.iter().map(|(p, q)| q.0 - p.0).collect());
    let dy = median(pairs.iter().map(|(p, q)| q.1 - p.1).collect());
    ((1.0, 0.0), (dx, dy))
}
