//! Depth maps: [`LocalDepth`] implements `DepthService` on the depth model
//! (Depth Anything V2 Small, `flikk-models`), reading frames through the
//! media engine as the viewer does (the proxy when there is one), numbered
//! on the media's rate.
//!
//! Each source frame is estimated by itself at the model's size (518 on
//! the longer side), spread over its own range (the 2nd to the 98th
//! percentile of its disparity: the model's answer has an unknown scale
//! and shift, so this is what makes frames comparable), turned into
//! distance and kept at 384 on the longer side, by the media's
//! fingerprint, the model and the frame, in memory and under
//! `paths::cache()/depth/` ([`store`]).
//!
//! A frame's map as shown is smoothed over time with the two frames on
//! either side: their distances averaged, weighted by how near in time
//! they are and, pixel by pixel, by how well they agree with this frame
//! once fitted onto it (so what moves keeps this frame's depth). Averaging
//! the frames' own spreads also steadies the range, which is what flickers
//! most. With every neighbour made the map is fixed, so renders, which
//! make them all first ([`DepthService::complete`]), match the viewer.
//!
//! Asking for a frame queues it, then the next two seconds and the frames
//! before it that smoothing reads, ahead of whatever was queued before, on
//! one worker thread; playback so keeps finding its maps made when the
//! machine is fast enough, and shows the nearest made frame's map
//! meanwhile. A frame that fails is not asked for again for
//! [`RETRY_AFTER`]; renders try it again all the same.
//!
//! On the GPU, the model takes turns there (`flikk_os::gpu`), a frame
//! each: the worker's frames are for the picture on screen, so they never
//! wait (watched); [`DepthService::complete`] lists its frames as a job,
//! "Depth: " and the file, at the level its caller asks for.

mod store;

use crate::engine::Claims;
use flikk_core::depth::{DepthMap, DepthService, DepthStatus};
use flikk_core::model::{Fingerprint, MediaItem, MediaKind};
use flikk_core::services::{Frame, FrameStream, MediaService, ServiceError, ServiceResult};
use flikk_core::time::{Rate, Time, TimeRange};
use flikk_models::depth::{DepthEstimator, Disparity};
use flikk_models::{Model, Task as ModelTask};
use flikk_os::gpu::{self, Level, Unit};
use flikk_os::sync::{lock, panic_message};
use std::collections::{HashMap, HashSet, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use store::Raw;

/// Widest frame read for the model, which sees 518 on the longer side.
const DECODE_WIDTH: u32 = 640;
/// Longer side of the maps kept.
const KEPT_SIDE: usize = 384;
/// Frames on either side a map is smoothed with.
const SMOOTH: i64 = 2;
/// How far apart in frames neighbours count half as much, about.
const SMOOTH_TIME: f32 = 1.2;
/// How far (in distance) a fitted neighbour may differ from the frame and
/// still count, about.
const SMOOTH_AGREE: f32 = 0.08;
/// Seconds made ahead of the frame asked for.
const AHEAD_SECONDS: i64 = 2;
/// Frames away a made map may stand in for one not made yet.
const NEAREST: i64 = 12;
/// Frames queued at most.
const WANTED_MAX: usize = 600;
/// Maps kept in memory, as made, and smoothed.
const RAW_KEPT: usize = 128;
const SMOOTHED_KEPT: usize = 32;
/// How long a frame that failed is left before it is asked for again.
const RETRY_AFTER: Duration = Duration::from_secs(30);

/// A frame's map: media, model, frame.
type Key = (Fingerprint, &'static str, i64);

/// The [`DepthService`] that runs the model on this machine.
pub struct LocalDepth {
    shared: Arc<Shared>,
}

struct Shared {
    media: Arc<dyn MediaService>,
    /// The model, loaded on first use; it runs one frame at a time.
    runner: Mutex<Option<(&'static str, Arc<dyn DepthEstimator>)>>,
    raw: Mutex<VecDeque<(Key, Arc<Raw>)>>,
    /// Keys looked for on disk and not there.
    absent: Mutex<HashSet<Key>>,
    smoothed: Mutex<VecDeque<(Key, DepthMap)>>,
    wanted: Mutex<Wanted>,
    /// Frames being made now.
    making: Claims<Key>,
    /// Frames that failed: why, and when.
    failed: Mutex<HashMap<Key, (String, Instant)>>,
    version: AtomicU64,
    seconds_per_frame: Mutex<Option<f32>>,
}

#[derive(Default)]
struct Wanted {
    /// Frames to make, the most wanted first.
    queue: VecDeque<(Key, MediaItem)>,
    /// The worker thread is running.
    working: bool,
}

fn runner(model: &'static Model) -> Option<Arc<dyn DepthEstimator>> {
    #[cfg(feature = "depth")]
    {
        Some(Arc::from(flikk_models::depth::runner(model)))
    }
    #[cfg(not(feature = "depth"))]
    {
        let _ = model;
        None
    }
}

impl LocalDepth {
    pub fn new(media: Arc<dyn MediaService>) -> Self {
        Self {
            shared: Arc::new(Shared {
                media,
                runner: Mutex::default(),
                raw: Mutex::default(),
                absent: Mutex::default(),
                smoothed: Mutex::default(),
                wanted: Mutex::default(),
                making: Claims::default(),
                failed: Mutex::default(),
                version: AtomicU64::new(0),
                seconds_per_frame: Mutex::default(),
            }),
        }
    }

    /// Whether this build makes depth maps (the `depth` feature).
    pub fn in_build() -> bool {
        cfg!(feature = "depth")
    }
}

/// The rate `item`'s frames are numbered on.
fn rate(item: &MediaItem) -> Rate {
    item.info
        .as_ref()
        .and_then(|i| i.video.as_ref())
        .map(|v| v.rate)
        .unwrap_or_default()
}

fn is_video(item: &MediaItem) -> bool {
    item.kind != MediaKind::Image
}

/// The frame of `item` showing at source time `t`.
fn frame_of(item: &MediaItem, t: Time) -> i64 {
    if is_video(item) {
        t.max(Time::ZERO).to_frame(rate(item))
    } else {
        0
    }
}

/// The last frame of `item`: unknown lengths end at `fallback`.
fn last_frame(item: &MediaItem, fallback: i64) -> i64 {
    if !is_video(item) {
        return 0;
    }
    match item.info.as_ref().map(|i| i.duration) {
        Some(d) if d > Time::ZERO => (d - Time(1)).to_frame(rate(item)).max(0),
        _ => fallback.max(0),
    }
}

fn time_of(item: &MediaItem, frame: i64) -> Time {
    if is_video(item) {
        Time::from_frame(frame, rate(item))
    } else {
        Time::ZERO
    }
}

/// The frames `range` of `item` shows, and those smoothing them reads.
fn frames_for(item: &MediaItem, range: TimeRange) -> (i64, i64) {
    let lo = frame_of(item, range.start);
    let hi = frame_of(item, (range.end - Time(1)).max(range.start)).max(lo);
    let last = last_frame(item, hi + SMOOTH);
    ((lo - SMOOTH).clamp(0, last), (hi + SMOOTH).clamp(0, last))
}

/// The value at quantile `q` (0..1) of `values`, from a sample.
fn quantile(sample: &[f32], q: f32) -> f32 {
    if sample.is_empty() {
        return 0.0;
    }
    let i = ((sample.len() - 1) as f32 * q).round() as usize;
    sample[i.min(sample.len() - 1)]
}

/// The model's disparity as kept: spread over its 2nd to 98th percentile,
/// as distance (0 nearest), resampled to [`KEPT_SIDE`] on the longer side.
fn normalize(d: &Disparity) -> Raw {
    let mut sample: Vec<f32> = d
        .values
        .iter()
        .step_by(3)
        .copied()
        .filter(|v| v.is_finite())
        .collect();
    sample.sort_by(f32::total_cmp);
    let (lo, hi) = (quantile(&sample, 0.02), quantile(&sample, 0.98));
    let span = (hi - lo).max(1e-6);
    let (sw, sh) = (d.width.max(1), d.height.max(1));
    let k = KEPT_SIDE as f32 / sw.max(sh) as f32;
    let (w, h) = (
        ((sw as f32 * k).round() as usize).max(1),
        ((sh as f32 * k).round() as usize).max(1),
    );
    let at = |x: usize, y: usize| d.values.get(y * sw + x).copied().unwrap_or(lo);
    let mut values = Vec::with_capacity(w * h);
    for y in 0..h {
        let fy = ((y as f32 + 0.5) * sh as f32 / h as f32 - 0.5).clamp(0.0, (sh - 1) as f32);
        let (y0, ty) = (fy.floor() as usize, fy.fract());
        let y1 = (y0 + 1).min(sh - 1);
        for x in 0..w {
            let fx = ((x as f32 + 0.5) * sw as f32 / w as f32 - 0.5).clamp(0.0, (sw - 1) as f32);
            let (x0, tx) = (fx.floor() as usize, fx.fract());
            let x1 = (x0 + 1).min(sw - 1);
            let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * tx;
            let bottom = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * tx;
            let disparity = top + (bottom - top) * ty;
            let near = ((disparity - lo) / span).clamp(0.0, 1.0);
            values.push(((1.0 - near) * 255.0).round() as u8);
        }
    }
    Raw {
        width: w,
        height: h,
        values,
    }
}

/// `b` fitted onto `a` by least squares, as (scale, shift), on a sample of
/// pixels.
fn fit(a: &Raw, b: &Raw) -> (f32, f32) {
    let (mut n, mut sa, mut sb, mut sbb, mut sab) = (0f32, 0f32, 0f32, 0f32, 0f32);
    for (va, vb) in a.values.iter().zip(&b.values).step_by(7) {
        let (x, y) = (*vb as f32 / 255.0, *va as f32 / 255.0);
        n += 1.0;
        sa += y;
        sb += x;
        sbb += x * x;
        sab += x * y;
    }
    if n < 2.0 {
        return (1.0, 0.0);
    }
    let var = sbb / n - (sb / n).powi(2);
    let cov = sab / n - (sa / n) * (sb / n);
    let scale = if var > 1e-5 {
        (cov / var).clamp(0.5, 2.0)
    } else {
        1.0
    };
    (scale, sa / n - scale * sb / n)
}

/// `center`'s map smoothed with `neighbours` (frames away, map) of the
/// same size.
fn smooth(center: &Raw, neighbours: &[(i64, Arc<Raw>)]) -> DepthMap {
    let fits: Vec<(f32, (f32, f32), &Raw)> = neighbours
        .iter()
        .map(|(d, raw)| {
            let t = *d as f32 / SMOOTH_TIME;
            ((-0.5 * t * t).exp(), fit(center, raw), raw.as_ref())
        })
        .collect();
    let values: Vec<u8> = (0..center.values.len())
        .map(|p| {
            let c = center.values[p] as f32 / 255.0;
            let (mut sum, mut total) = (c, 1.0f32);
            for (w, (scale, shift), raw) in &fits {
                let v = raw.values[p] as f32 / 255.0;
                let off = (scale * v + shift - c) / SMOOTH_AGREE;
                let w = w * (-off * off).exp();
                sum += w * v;
                total += w;
            }
            ((sum / total).clamp(0.0, 1.0) * 255.0).round() as u8
        })
        .collect();
    DepthMap {
        width: center.width as u32,
        height: center.height as u32,
        values: values.into(),
    }
}

fn found<V: Clone>(list: &Mutex<VecDeque<(Key, V)>>, key: &Key) -> Option<V> {
    let list = lock(list);
    list.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

fn keep<V>(list: &Mutex<VecDeque<(Key, V)>>, key: Key, value: V, most: usize) {
    let mut list = lock(list);
    list.retain(|(k, _)| *k != key);
    list.push_front((key, value));
    list.truncate(most);
}

impl Shared {
    fn bump(&self) {
        self.version.fetch_add(1, Ordering::SeqCst);
    }

    fn model_here() -> bool {
        LocalDepth::in_build() && ModelTask::Depth.model().is_downloaded()
    }

    /// The map of `key` as made, from memory or disk.
    fn raw(&self, key: &Key) -> Option<Arc<Raw>> {
        if let Some(raw) = found(&self.raw, key) {
            return Some(raw);
        }
        if lock(&self.absent).contains(key) {
            return None;
        }
        match store::load(&store::path(&key.0, key.1, key.2)) {
            Some(raw) => {
                let raw = Arc::new(raw);
                keep(&self.raw, key.clone(), raw.clone(), RAW_KEPT);
                Some(raw)
            }
            None => {
                lock(&self.absent).insert(key.clone());
                None
            }
        }
    }

    /// Whether `key` is made, without reading it.
    fn made(&self, key: &Key) -> bool {
        if found(&self.raw, key).is_some() {
            return true;
        }
        if lock(&self.absent).contains(key) {
            return false;
        }
        let here = store::path(&key.0, key.1, key.2).is_file();
        if !here {
            lock(&self.absent).insert(key.clone());
        }
        here
    }

    /// The runner of `model`, made anew when another model was picked;
    /// `None` in a build without depth maps.
    fn runner(&self, model: &'static Model) -> Option<Arc<dyn DepthEstimator>> {
        let mut slot = lock(&self.runner);
        if slot.as_ref().is_none_or(|(id, _)| *id != model.id) {
            *slot = runner(model).map(|r| (model.id, r));
        }
        slot.as_ref().map(|(_, runner)| runner.clone())
    }

    /// Run `runner` on `picture`, in `unit`'s turn on the GPU.
    fn estimate(
        &self,
        runner: Option<&dyn DepthEstimator>,
        picture: &Frame,
        unit: Unit<'_>,
    ) -> anyhow::Result<Raw> {
        let Some(runner) = runner else {
            anyhow::bail!("Depth maps are not in this build.");
        };
        let start = Instant::now();
        let disparity = runner.estimate(picture, unit)?;
        *lock(&self.seconds_per_frame) = Some(start.elapsed().as_secs_f32());
        Ok(normalize(&disparity))
    }

    fn store(&self, key: &Key, raw: Raw) {
        if let Err(e) = store::store(&store::path(&key.0, key.1, key.2), &raw) {
            eprintln!("flikk-analysis: {e:#}");
        }
        keep(&self.raw, key.clone(), Arc::new(raw), RAW_KEPT);
        lock(&self.absent).remove(key);
        lock(&self.failed).remove(key);
        self.bump();
    }

    fn fail(&self, key: &Key, message: String) {
        lock(&self.failed).insert(key.clone(), (message, Instant::now()));
        self.bump();
    }

    /// Whether `key` failed less than [`RETRY_AFTER`] ago.
    fn failed_lately(&self, key: &Key) -> bool {
        lock(&self.failed)
            .get(key)
            .is_some_and(|(_, at)| at.elapsed() < RETRY_AFTER)
    }

    /// Make `key` from `picture` unless it is made, the model running in
    /// `unit`'s turn on the GPU. When another thread is making it, waits
    /// for that (`wait`) or leaves it to it. Cancelled, it fails without
    /// counting as a failure.
    fn make(
        &self,
        key: &Key,
        model: &'static Model,
        wait: bool,
        unit: Unit<'_>,
        picture: ServiceResult<Frame>,
    ) -> Result<(), String> {
        let runner = self.runner(model);
        // The turn before the claim: a thread that claimed a frame never
        // waits for its turn, so the frames on screen never wait behind
        // a background render's. The model's own turn is this one.
        let _turn = match &runner {
            Some(runner) if runner.on_gpu() => Some(unit.take().ok_or("canceled")?),
            _ => None,
        };
        let claimed = self.making.claim(
            key,
            || self.made(key).then_some(()),
            || (!wait).then_some(()),
            None,
        );
        let Ok(_claim) = claimed else {
            return Ok(());
        };
        let result = catch_unwind(AssertUnwindSafe(|| {
            let picture = picture.map_err(|e| e.to_string())?;
            self.estimate(runner.as_deref(), &picture, unit)
                .map_err(|e| format!("{e:#}"))
        }))
        .unwrap_or_else(|payload| {
            // Load the model anew rather than trust what it was left as.
            *lock(&self.runner) = None;
            Err(panic_message("making the depth map", payload.as_ref()))
        });
        match result {
            Ok(raw) => {
                self.store(key, raw);
                Ok(())
            }
            Err(e) if unit.cancelled() => Err(e),
            Err(e) => {
                self.fail(key, e.clone());
                Err(e)
            }
        }
    }

    /// Queue `frames` of `item`, most wanted first, ahead of the rest.
    fn want(self: &Arc<Self>, item: &MediaItem, model: &'static Model, frames: Vec<i64>) {
        let Some(fp) = item.fingerprint.clone() else {
            return;
        };
        let keys: Vec<Key> = frames
            .into_iter()
            .map(|f| (fp.clone(), model.id, f))
            .filter(|k| !self.failed_lately(k) && !self.made(k))
            .collect();
        if keys.is_empty() {
            return;
        }
        let mut wanted = lock(&self.wanted);
        let new: HashSet<&Key> = keys.iter().collect();
        wanted.queue.retain(|(k, _)| !new.contains(k));
        for key in keys.iter().rev() {
            wanted.queue.push_front((key.clone(), item.clone()));
        }
        wanted.queue.truncate(WANTED_MAX);
        if !wanted.working {
            wanted.working = true;
            let shared = self.clone();
            let spawned = std::thread::Builder::new()
                .name("flikk-depth".into())
                .spawn(move || shared.work());
            if spawned.is_err() {
                wanted.working = false;
            }
        }
    }

    /// The worker: make what is wanted until nothing is.
    fn work(self: Arc<Self>) {
        let _working = Working(&self);
        loop {
            let next = {
                let mut wanted = lock(&self.wanted);
                match wanted.queue.pop_front() {
                    Some(next) => next,
                    None => {
                        wanted.working = false;
                        return;
                    }
                }
            };
            let (key, item) = next;
            let Some(model) = ModelTask::Depth
                .models()
                .iter()
                .find(|m| m.id == key.1)
                .copied()
            else {
                continue;
            };
            // It may have failed since it was queued (a render tried it),
            // or be made or being made by one meanwhile.
            if self.failed_lately(&key) || self.made(&key) || self.making.held(&key) {
                continue;
            }
            let picture = catch_unwind(AssertUnwindSafe(|| {
                self.media.frame(&item, time_of(&item, key.2), DECODE_WIDTH)
            }))
            .unwrap_or_else(|payload| {
                Err(ServiceError(panic_message(
                    "making the depth map",
                    payload.as_ref(),
                )))
            });
            // For the frames on screen: never behind other GPU work.
            let _ = self.make(&key, model, false, Unit::Watched, picture);
        }
    }

    /// Frame `frame`'s map, smoothed with the neighbours made.
    fn smoothed(
        &self,
        fp: &Fingerprint,
        model: &'static str,
        frame: i64,
        last: i64,
    ) -> Option<DepthMap> {
        let key = (fp.clone(), model, frame);
        if let Some(map) = found(&self.smoothed, &key) {
            return Some(map);
        }
        let center = self.raw(&key)?;
        let around: Vec<i64> = (-SMOOTH..=SMOOTH)
            .filter(|d| *d != 0 && (0..=last).contains(&(frame + d)))
            .collect();
        let neighbours: Vec<(i64, Arc<Raw>)> = around
            .iter()
            .filter_map(|d| Some((*d, self.raw(&(fp.clone(), model, frame + d))?)))
            .filter(|(_, r)| r.width == center.width && r.height == center.height)
            .collect();
        let map = smooth(&center, &neighbours);
        // Fixed once every neighbour is in.
        if neighbours.len() == around.len() {
            keep(&self.smoothed, key, map.clone(), SMOOTHED_KEPT);
        }
        Some(map)
    }
}

/// Marks the worker stopped should it panic past its frames.
struct Working<'a>(&'a Shared);

impl Drop for Working<'_> {
    fn drop(&mut self) {
        // A worker that returns marks itself stopped as it finds nothing.
        if std::thread::panicking() {
            lock(&self.0.wanted).working = false;
        }
    }
}

/// Frames of `item` read in order from one stream; decoded alone where it
/// ends short (variable-rate files can).
struct Reader<'a> {
    media: &'a dyn MediaService,
    item: &'a MediaItem,
    stream: Option<(i64, Box<dyn FrameStream>)>,
}

impl Reader<'_> {
    fn get(&mut self, frame: i64) -> ServiceResult<Frame> {
        if !is_video(self.item) {
            return self.media.frame(self.item, Time::ZERO, DECODE_WIDTH);
        }
        if self.stream.as_ref().is_none_or(|(next, _)| *next != frame) {
            let stream = self
                .media
                .playback(self.item, time_of(self.item, frame), DECODE_WIDTH)?;
            self.stream = Some((frame, stream));
        }
        let (_, mut stream) = self.stream.take().expect("opened above");
        match stream.next_frame()? {
            Some((_, picture)) => {
                self.stream = Some((frame + 1, stream));
                Ok(picture)
            }
            None => self
                .media
                .frame(self.item, time_of(self.item, frame), DECODE_WIDTH),
        }
    }
}

impl DepthService for LocalDepth {
    fn available(&self) -> Result<(), String> {
        if !Self::in_build() {
            return Err("Depth maps are not in this build.".into());
        }
        let model = ModelTask::Depth.model();
        if !model.is_downloaded() {
            return Err("Download it in Settings → Integrations.".into());
        }
        Ok(())
    }

    fn depth(&self, item: &MediaItem, t: Time) -> Option<DepthMap> {
        let fp = item.fingerprint.clone()?;
        let model = ModelTask::Depth.model();
        let shared = &self.shared;
        let frame = frame_of(item, t);
        let last = last_frame(item, frame + SMOOTH);
        let frame = frame.min(last);
        if Shared::model_here() {
            let ahead =
                (AHEAD_SECONDS * rate(item).num as i64 / rate(item).den.max(1) as i64).max(1);
            let wanted: Vec<i64> = (frame..=(frame + ahead).min(last))
                .chain((0.max(frame - SMOOTH)..frame).rev())
                .collect();
            shared.want(item, model, wanted);
        }
        if let Some(map) = shared.smoothed(&fp, model.id, frame, last) {
            return Some(map);
        }
        (1..=NEAREST)
            .flat_map(|d| [frame - d, frame + d])
            .filter(|f| (0..=last).contains(f))
            .find(|f| shared.made(&(fp.clone(), model.id, *f)))
            .and_then(|f| shared.smoothed(&fp, model.id, f, last))
    }

    fn status(&self, item: &MediaItem, range: TimeRange) -> DepthStatus {
        let shared = &self.shared;
        let Some(fp) = item.fingerprint.clone() else {
            return DepthStatus {
                error: Some("The media has no fingerprint yet.".into()),
                ..Default::default()
            };
        };
        let model = ModelTask::Depth.model();
        let (lo, hi) = frames_for(item, range);
        // The latest failure among these frames.
        let error = self.available().err().or_else(|| {
            let failed = lock(&shared.failed);
            (lo..=hi)
                .filter_map(|f| failed.get(&(fp.clone(), model.id, f)))
                .max_by_key(|(_, at)| *at)
                .map(|(why, _)| why.clone())
        });
        let made = (lo..=hi)
            .filter(|f| shared.made(&(fp.clone(), model.id, *f)))
            .count();
        DepthStatus {
            frames: (hi - lo + 1) as usize,
            made,
            seconds_per_frame: *lock(&shared.seconds_per_frame),
            error,
        }
    }

    fn complete(
        &self,
        item: &MediaItem,
        range: TimeRange,
        level: Level,
        progress: &dyn Fn(f32),
        cancel: &AtomicBool,
    ) -> ServiceResult<()> {
        self.available().map_err(ServiceError)?;
        let shared = &self.shared;
        let fp = item
            .fingerprint
            .clone()
            .ok_or_else(|| ServiceError("the media has no fingerprint yet".into()))?;
        let model = ModelTask::Depth.model();
        let (lo, hi) = frames_for(item, range);
        let missing: Vec<i64> = (lo..=hi)
            .filter(|f| !shared.made(&(fp.clone(), model.id, *f)))
            .collect();
        let mut reader = Reader {
            media: shared.media.as_ref(),
            item,
            stream: None,
        };
        let job = gpu::Job::new(format!("Depth: {}", crate::file_name(&item.path)), level);
        let unit = Unit::Job { job: &job, cancel };
        for (i, frame) in missing.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return Err(ServiceError("canceled".into()));
            }
            let key = (fp.clone(), model.id, *frame);
            // Read here even when made meanwhile, so the stream reads on.
            let picture =
                catch_unwind(AssertUnwindSafe(|| reader.get(*frame))).unwrap_or_else(|payload| {
                    reader.stream = None;
                    Err(ServiceError(panic_message(
                        "making the depth map",
                        payload.as_ref(),
                    )))
                });
            // Tried again even when it failed lately: a render waits for it.
            match shared.make(&key, model, true, unit, picture) {
                Err(_) if unit.cancelled() => return Err(ServiceError("canceled".into())),
                Err(why) => return Err(ServiceError(format!("frame {frame}: {why}"))),
                Ok(()) => {}
            }
            let done = (i + 1) as f32 / missing.len() as f32;
            job.progress(done);
            progress(done);
        }
        Ok(())
    }

    fn version(&self) -> u64 {
        self.shared.version.load(Ordering::SeqCst)
    }
}
