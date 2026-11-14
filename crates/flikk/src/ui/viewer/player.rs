//! Real-time video for the program monitor. A background thread follows the
//! playhead and composes each sequence frame there
//! ([`flikk_core::compose::submit_program`]) from one playback stream per
//! visible layer ([`Streams`]: reopened at cuts and jumps, decoded and
//! dropped when a clip runs faster than 1×, held when slower), and
//! publishes the newest picture as a GPUI image.
//!
//! Frames are pipelined: frame N+1 is decoded, uploaded and submitted
//! before frame N is read back, so the CPU and the GPU work at the same
//! time. Each frame's cost (its share of the thread's busy time) sets the
//! preview resolution ([`Quality`]); full-resolution frames go into the
//! viewer's [`FrameCache`], and cached frames are shown without composing.
//! [`Player::render_ahead`] runs the same loop over the frames after a
//! paused playhead, only filling the cache. Frames the background render
//! put on disk ([`crate::render_cache`]) are read instead of composed.
//!
//! Playback is watched GPU work ([`gpu::watching`]): it never waits, and
//! background work keeps off the GPU meanwhile. Rendering ahead is
//! background work, one frame per turn, and not listed: it starts over
//! each time the playhead rests.

use super::PlayKey;
use super::cache::SharedCache;
use crate::render_cache::{self, Globals, Plan};
use flikk_core::Time;
use flikk_core::compose::{self, Pending, Streams};
use flikk_core::model::Project;
use flikk_core::services::{Frame, PixelOrder, Services};
use flikk_os::gpu;
use gpui_kit::RenderImage;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Lowest preview resolution: 1 / 2^MAX_SHIFT of the viewer width.
const MAX_SHIFT: u8 = 2;
/// Frames after a resolution change (or the start) that are not measured:
/// streams open and textures are allocated.
const SETTLE: u32 = 4;
/// Measured frames in a row under half the frame time before a step back
/// up is tried; doubled after each try that fails, up to [`MAX_CALM`].
const CALM: u32 = 60;
const MAX_CALM: u32 = 960;
/// Frames a step up must last to count as a success.
const PROBE: u32 = 90;

#[derive(Default)]
struct Shared {
    target: AtomicI64,
    stop: AtomicBool,
    /// Newest picture not yet taken by the UI.
    latest: Mutex<Option<Arc<RenderImage>>>,
}

pub struct Player {
    shared: Arc<Shared>,
}

/// What a player thread needs besides its shared state.
struct Job {
    services: Services,
    project: Arc<Project>,
    key: PlayKey,
    cache: SharedCache,
    /// What the frames on disk share, when they can stand in.
    disk: Option<Globals>,
    /// Their plan, made on the thread.
    plan: Option<Arc<Plan>>,
}

impl Player {
    /// Compose `project` from `from`, `key.width` pixels wide at most;
    /// `shift` holds the preview resolution (see [`Quality`]) across
    /// playbacks.
    pub fn start(
        services: Services,
        project: Arc<Project>,
        from: Time,
        key: PlayKey,
        cache: SharedCache,
        shift: Arc<AtomicU8>,
        disk: Option<Globals>,
    ) -> Self {
        let shared = Arc::new(Shared::default());
        shared.target.store(from.0, Ordering::Relaxed);
        let worker = shared.clone();
        let job = Job {
            services,
            project,
            key,
            cache,
            disk,
            plan: None,
        };
        let _ = thread::Builder::new()
            .name("flikk-player".into())
            .spawn(move || play(worker, job, Quality::new(shift)));
        Self { shared }
    }

    /// Compose the frames in `frames` at full resolution into `cache`, then
    /// stop.
    pub fn render_ahead(
        services: Services,
        project: Arc<Project>,
        frames: std::ops::Range<i64>,
        key: PlayKey,
        cache: SharedCache,
        disk: Option<Globals>,
    ) -> Self {
        let shared = Arc::new(Shared::default());
        let worker = shared.clone();
        let job = Job {
            services,
            project,
            key,
            cache,
            disk,
            plan: None,
        };
        let _ = thread::Builder::new()
            .name("flikk-render-ahead".into())
            .spawn(move || ahead(worker, job, frames));
        Self { shared }
    }

    pub fn set_target(&self, t: Time) {
        self.shared.target.store(t.0, Ordering::Relaxed);
    }

    /// The newest picture since the last call.
    pub fn take(&self) -> Option<Arc<RenderImage>> {
        self.shared.latest.lock().ok()?.take()
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

/// RGBA frame → BGRA `RenderImage`.
pub fn to_image(frame: &Frame) -> Option<Arc<RenderImage>> {
    let mut bgra = frame.rgba.to_vec();
    for px in bgra.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    image_of(frame.width, frame.height, bgra)
}

/// A frame read back in [`PixelOrder::Bgra`] as a `RenderImage`.
pub fn bgra_image(frame: &Frame) -> Option<Arc<RenderImage>> {
    image_of(frame.width, frame.height, frame.rgba.to_vec())
}

fn image_of(width: u32, height: u32, bgra: Vec<u8>) -> Option<Arc<RenderImage>> {
    let buffer = image::RgbaImage::from_raw(width, height, bgra)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}

/// A frame between submit and display.
enum InFlight<'a> {
    Cached(Arc<RenderImage>),
    Composing {
        frame: i64,
        /// Composed at the viewer's full width (so it can be cached).
        full: bool,
        pending: Pending<'a>,
        /// Busy time spent on it so far: decoding, uploading, submitting.
        cost: Duration,
    },
}

impl Job {
    fn plan(&mut self) {
        self.plan = self
            .disk
            .map(|globals| render_cache::plan_for(&self.project, globals));
    }

    /// Frame `frame` from disk, when it is there and large enough: up to a
    /// third smaller than the viewer is fine while playing.
    fn disk_image(&self, frame: i64) -> Option<Arc<RenderImage>> {
        let plan = self.plan.as_ref()?;
        let width = plan.size.0;
        (self.key.width <= width + width / 3)
            .then(|| render_cache::image(plan, frame))
            .flatten()
    }

    fn on_disk(&self, frame: i64) -> bool {
        self.plan
            .as_ref()
            .and_then(|p| p.key(frame))
            .is_some_and(|k| render_cache::store().contains(&k))
    }

    /// Start frame `frame`: from the cache, or composed at
    /// `width >> shift`.
    fn begin<'a>(
        &'a self,
        frame: i64,
        shift: u8,
        streams: &mut Streams,
        cache_hits: bool,
    ) -> InFlight<'a> {
        if cache_hits
            && let Some(image) = self
                .cache
                .lock()
                .ok()
                .and_then(|mut c| c.get(self.key, frame))
        {
            return InFlight::Cached(image);
        }
        if cache_hits && let Some(image) = self.disk_image(frame) {
            return InFlight::Cached(image);
        }
        let started = Instant::now();
        let rate = self.project.timeline.settings.rate;
        let size = compose::size_for_width(&self.project, self.key.width >> shift);
        let pending = compose::submit_program(
            &self.project,
            Time::from_frame(frame, rate),
            size,
            &self.services,
            streams,
            PixelOrder::Bgra,
            self.key.masks,
        );
        InFlight::Composing {
            frame,
            full: shift == 0,
            pending,
            cost: started.elapsed(),
        }
    }

    /// Wait for `flight`'s picture; full-resolution ones go into the cache.
    /// Also returns what the frame cost, when it was composed.
    fn end(&self, flight: InFlight<'_>) -> (Option<Arc<RenderImage>>, Option<Duration>) {
        match flight {
            InFlight::Cached(image) => (Some(image), None),
            InFlight::Composing {
                frame,
                full,
                pending,
                cost,
            } => {
                let started = Instant::now();
                let picture = pending.wait();
                let cost = cost + started.elapsed();
                let image = bgra_image(&picture);
                if full
                    && let Some(image) = &image
                    && let Ok(mut cache) = self.cache.lock()
                {
                    cache.insert(self.key, frame, image.clone(), picture.rgba.len());
                }
                (image, Some(cost))
            }
        }
    }
}

fn play(shared: Arc<Shared>, mut job: Job, mut quality: Quality) {
    job.plan();
    let rate = job.project.timeline.settings.rate;
    let budget = rate.frame_duration().as_secs_f64();
    let mut streams = Streams::new(job.services.media.clone(), job.key.width, rate);
    let mut shown: Option<i64> = None;
    let mut flight: Option<InFlight> = None;
    let publish = |(image, cost): (Option<Arc<RenderImage>>, Option<Duration>),
                   quality: &mut Quality| {
        if let Some(cost) = cost {
            quality.record(cost.as_secs_f64(), budget);
        }
        if let Some(image) = image
            && let Ok(mut latest) = shared.latest.lock()
        {
            *latest = Some(image);
        }
    };
    while !shared.stop.load(Ordering::Relaxed) {
        let _watching = gpu::watching();
        let frame = Time(shared.target.load(Ordering::Relaxed)).to_frame(rate);
        if shown == Some(frame) {
            match flight.take() {
                Some(f) => publish(job.end(f), &mut quality),
                None => thread::sleep(Duration::from_millis(2)),
            }
            continue;
        }
        shown = Some(frame);
        let next = job.begin(frame, quality.shift(), &mut streams, true);
        if let Some(previous) = flight.replace(next) {
            publish(job.end(previous), &mut quality);
        }
    }
}

fn ahead(shared: Arc<Shared>, mut job: Job, frames: std::ops::Range<i64>) {
    job.plan();
    let rate = job.project.timeline.settings.rate;
    let mut streams = Streams::new(job.services.media.clone(), job.key.width, rate);
    let mut flight: Option<InFlight> = None;
    for frame in frames {
        if shared.stop.load(Ordering::Relaxed) {
            return;
        }
        let cached =
            job.cache.lock().is_ok_and(|c| c.contains(job.key, frame)) || job.on_disk(frame);
        if cached {
            continue;
        }
        let Some(_turn) = gpu::turn(gpu::Level::Background, &shared.stop) else {
            return;
        };
        let next = job.begin(frame, 0, &mut streams, false);
        if let Some(previous) = flight.replace(next) {
            job.end(previous);
        }
    }
    if let Some(last) = flight
        && let Some(_turn) = gpu::turn(gpu::Level::Background, &shared.stop)
    {
        job.end(last);
    }
}

/// Preview resolution while playing, as a right shift of the viewer width:
/// one step down when the average frame costs more than the frame time; one
/// step up, as a probe, after enough frames under half of it. A probe that
/// steps back down within [`PROBE`] frames doubles the wait for the next.
struct Quality {
    shift: Arc<AtomicU8>,
    /// Running average of frame cost, seconds.
    average: Option<f64>,
    /// Measured frames in a row under half the frame time.
    calm: u32,
    /// Calm frames needed before the next step up.
    patience: u32,
    /// Frames since the last step up, while it is still a probe.
    probe: Option<u32>,
    settle: u32,
}

impl Quality {
    fn new(shift: Arc<AtomicU8>) -> Self {
        Self {
            shift,
            average: None,
            calm: 0,
            patience: CALM,
            probe: None,
            settle: SETTLE,
        }
    }

    fn shift(&self) -> u8 {
        self.shift.load(Ordering::Relaxed).min(MAX_SHIFT)
    }

    fn record(&mut self, cost: f64, budget: f64) {
        if self.settle > 0 {
            self.settle -= 1;
            return;
        }
        if let Some(frames) = self.probe.as_mut() {
            *frames += 1;
            if *frames >= PROBE {
                self.probe = None;
                self.patience = CALM;
            }
        }
        let average = self.average.map_or(cost, |a| a * 0.8 + cost * 0.2);
        self.average = Some(average);
        let shift = self.shift();
        if average > budget && shift < MAX_SHIFT {
            if self.probe.take().is_some() {
                self.patience = (self.patience * 2).min(MAX_CALM);
            }
            self.set(shift + 1);
        } else if average < budget * 0.5 && shift > 0 {
            self.calm += 1;
            if self.calm >= self.patience {
                self.set(shift - 1);
                self.probe = Some(0);
            }
        } else {
            self.calm = 0;
        }
    }

    fn set(&mut self, shift: u8) {
        self.shift.store(shift, Ordering::Relaxed);
        self.average = None;
        self.calm = 0;
        self.settle = SETTLE;
    }
}
