//! Person masks: [`LocalPeople`] finds the people in every frame of a
//! clip with the matting model (MODNet, `koo-models`), reading frames
//! through the media engine as the viewer does (the proxy when there is
//! one) at the model's size, numbered on the media's rate.
//!
//! Frames are made in chunks of [`CHUNK`], kept by the media's
//! fingerprint, the model and the chunk, in memory and under
//! `paths::cache()/mattes/` ([`store`]), so a clip is matted once whatever
//! its masks' settings, and every person mask on the same media shares
//! them. One worker makes them, newest request first: a frame the viewer
//! shows asks for the frames from it on, a few seconds ahead, so playback
//! finds them made; a render asks for its whole range and waits. When the
//! newest request wants another chunk the worker leaves the one it is on
//! (keeping what it made, to go on from later).
//!
//! Each frame is eased toward the one before it where they barely differ
//! (most where they agree, not at all past [`MOTION`]), which stops the
//! flicker a per-frame model gives on still edges without dragging moving
//! ones. A failed media is not tried again until the model is downloaded
//! anew.

mod store;

use crate::lock;
use koo_core::ids::MaskId;
use koo_core::model::{Fingerprint, MediaItem, MediaKind};
use koo_core::person::PersonMatte;
use koo_core::services::{
    Frame, FrameStream, MediaService, PersonRef, ServiceError, ServiceResult, SubjectStatus,
};
use koo_core::time::{Rate, Time};
use koo_models::matting::Matter;
use koo_models::{Model, Task as ModelTask};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use store::Packed;

/// Frames made, kept and written together.
const CHUNK: i64 = 32;
/// How far past a frame the viewer shows its frames are made.
const AHEAD: f64 = 3.0;
/// How far from a frame without its matte the nearest made one is shown.
const NEAREST: f64 = 1.0;
/// Chunks kept in memory (made ones are read again from disk).
const CHUNKS_KEPT: usize = 96;
/// Mattes kept unpacked, for frames drawn again and again.
const UNPACKED_KEPT: usize = 8;
/// Short side of the frames read for the model, which sees 512.
const SHORT_SIDE: u32 = 512;
/// A change in coverage from one frame to the next past which a pixel
/// moved, and is not eased.
const MOTION: f32 = 0.25;

/// A media on a model.
type MediaKey = (Fingerprint, &'static str);
/// A chunk of a media's frames on a model.
type ChunkKey = (Fingerprint, &'static str, i64);

/// Who wants frames made.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Want {
    /// A person mask in a view, from the frame shown on.
    View(MaskId),
    /// A render, waiting.
    Render(u64),
}

struct Request {
    item: MediaItem,
    model: &'static Model,
    /// Frames wanted, first and last.
    lo: i64,
    hi: i64,
    /// Newer requests are served first.
    touched: u64,
}

/// The person masks' [`SubjectService`](koo_core::services::SubjectService)
/// half: see the module docs.
pub(crate) struct LocalPeople {
    shared: Arc<Shared>,
}

struct Shared {
    media: Arc<dyn MediaService>,
    state: Mutex<State>,
    wake: Condvar,
    version: AtomicU64,
}

#[derive(Default)]
struct State {
    /// Made frames by chunk, from its first frame on; a chunk left part of
    /// the way holds fewer.
    chunks: HashMap<ChunkKey, Vec<Packed>>,
    /// Chunks in memory, the latest used last.
    order: VecDeque<ChunkKey>,
    /// Chunks made whole, read from disk the first time.
    whole: HashMap<MediaKey, BTreeSet<i64>>,
    requests: HashMap<Want, Request>,
    clock: u64,
    /// The chunk the worker is on.
    busy: Option<ChunkKey>,
    /// Why a media failed, and whether the model was there then.
    failed: HashMap<MediaKey, (String, bool)>,
    unpacked: VecDeque<((ChunkKey, usize), PersonMatte)>,
    /// How long the latest frame took, on which model.
    seconds_per_frame: Option<(&'static str, f32)>,
    worker: bool,
}

/// A chunk to make: who wants it, the media, and where it stands.
struct Job {
    key: ChunkKey,
    item: MediaItem,
    model: &'static Model,
    /// Frames it holds when whole, and already made.
    len: usize,
    made: usize,
}

fn runner(model: &'static Model) -> Option<Box<dyn Matter>> {
    #[cfg(feature = "people")]
    {
        Some(koo_models::matting::runner(model))
    }
    #[cfg(not(feature = "people"))]
    {
        let _ = model;
        None
    }
}

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

/// The last frame of `item`, when its length is known.
fn last_frame(item: &MediaItem) -> Option<i64> {
    if !is_video(item) {
        return Some(0);
    }
    let duration = item.info.as_ref()?.duration;
    (duration > Time::ZERO).then(|| ((duration - Time(1)).to_frame(rate(item))).max(0))
}

/// The frames `person` shows, first and last.
fn frames(person: &PersonRef<'_>) -> (i64, i64) {
    let item = person.item;
    let lo = frame_of(item, person.range.start);
    let mut hi = frame_of(item, person.range.end - Time(1)).max(lo);
    if let Some(last) = last_frame(item) {
        hi = hi.min(last);
    }
    (lo.min(hi), hi)
}

/// Frames of `item` in `seconds`.
fn frames_in(item: &MediaItem, seconds: f64) -> i64 {
    if !is_video(item) {
        return 0;
    }
    let r = rate(item);
    (seconds * r.num as f64 / r.den.max(1) as f64).ceil() as i64
}

/// Widest frame read for the model: the short side at [`SHORT_SIDE`].
fn decode_width(item: &MediaItem) -> u32 {
    match item.info.as_ref().and_then(|i| i.video.as_ref()) {
        Some(v) if v.width > 0 && v.height > 0 => {
            let w = if v.width >= v.height {
                (SHORT_SIDE as f64 * v.width as f64 / v.height as f64).round() as u32
            } else {
                SHORT_SIDE
            };
            w.min(v.width).max(1)
        }
        _ => 1024,
    }
}

/// Frames chunk `chunk` holds when whole, for a media whose last frame is
/// `last`.
fn chunk_len(chunk: i64, last: Option<i64>) -> usize {
    let start = chunk * CHUNK;
    match last {
        Some(last) => (last - start + 1).clamp(0, CHUNK) as usize,
        None => CHUNK as usize,
    }
}

fn model_here() -> bool {
    cfg!(feature = "people") && ModelTask::Matting.model().is_downloaded()
}

impl State {
    /// The whole chunks of `key`, listed from disk the first time.
    fn whole(&mut self, key: &MediaKey) -> &mut BTreeSet<i64> {
        self.whole
            .entry(key.clone())
            .or_insert_with(|| store::list(&key.0, key.1))
    }

    /// The frames made of `key`, read from disk when it was made whole
    /// before.
    fn chunk(&mut self, key: &ChunkKey) -> Option<&Vec<Packed>> {
        if !self.chunks.contains_key(key) {
            let media = (key.0.clone(), key.1);
            if !self.whole(&media).contains(&key.2) {
                return None;
            }
            match store::load(&store::chunk_path(&key.0, key.1, key.2)) {
                Some(frames) => self.keep(key.clone(), frames),
                None => {
                    // Gone or unreadable: made again.
                    self.whole(&media).remove(&key.2);
                    return None;
                }
            }
        }
        self.touch(key);
        self.chunks.get(key)
    }

    fn keep(&mut self, key: ChunkKey, frames: Vec<Packed>) {
        self.chunks.insert(key.clone(), frames);
        self.touch(&key);
        while self.order.len() > CHUNKS_KEPT {
            // The oldest, unless the worker is on it.
            let Some(i) = self
                .order
                .iter()
                .position(|k| self.busy.as_ref() != Some(k))
            else {
                break;
            };
            if let Some(old) = self.order.remove(i) {
                self.chunks.remove(&old);
            }
        }
    }

    fn touch(&mut self, key: &ChunkKey) {
        if let Some(i) = self.order.iter().position(|k| k == key) {
            self.order.remove(i);
        }
        self.order.push_back(key.clone());
    }

    /// The matte of frame `frame`, unpacked.
    fn matte(&mut self, media: &MediaKey, frame: i64) -> Option<PersonMatte> {
        let key = (media.0.clone(), media.1, frame.div_euclid(CHUNK));
        let index = frame.rem_euclid(CHUNK) as usize;
        if let Some((_, m)) = self
            .unpacked
            .iter()
            .find(|(k, _)| k.0 == key && k.1 == index)
        {
            return Some(m.clone());
        }
        let m = self.chunk(&key)?.get(index)?.unpack()?;
        self.unpacked.push_back(((key, index), m.clone()));
        while self.unpacked.len() > UNPACKED_KEPT {
            self.unpacked.pop_front();
        }
        Some(m)
    }

    /// Why `media` failed, unless the model came since.
    fn failure(&mut self, media: &MediaKey) -> Option<String> {
        match self.failed.get(media) {
            Some((_, false)) if model_here() => {
                self.failed.remove(media);
                None
            }
            Some((message, _)) => Some(message.clone()),
            None => None,
        }
    }

    /// The chunk to make next: the first not whole of the newest request
    /// that has one. Requests with none left are let go.
    fn next(&mut self) -> Option<Job> {
        let mut wants: Vec<(u64, Want)> =
            self.requests.iter().map(|(w, r)| (r.touched, *w)).collect();
        wants.sort_by_key(|w| std::cmp::Reverse(w.0));
        for (_, want) in wants {
            let Some(r) = self.requests.get(&want) else {
                continue;
            };
            let (item, model, lo, hi) = (r.item.clone(), r.model, r.lo, r.hi);
            let Some(fp) = item.fingerprint.clone() else {
                self.requests.remove(&want);
                continue;
            };
            let media = (fp.clone(), model.id);
            if self.failure(&media).is_some() {
                self.requests.remove(&want);
                continue;
            }
            let last = last_frame(&item);
            let whole = self.whole(&media).clone();
            let missing = (lo.div_euclid(CHUNK)..=hi.div_euclid(CHUNK))
                .find(|c| !whole.contains(c) && chunk_len(*c, last) > 0);
            let Some(chunk) = missing else {
                if matches!(want, Want::View(_)) {
                    self.requests.remove(&want);
                }
                continue;
            };
            let key = (fp, model.id, chunk);
            let made = self.chunks.get(&key).map_or(0, Vec::len);
            return Some(Job {
                key,
                item,
                model,
                len: chunk_len(chunk, last),
                made,
            });
        }
        None
    }
}

impl LocalPeople {
    pub(crate) fn new(media: Arc<dyn MediaService>) -> Self {
        Self {
            shared: Arc::new(Shared {
                media,
                state: Mutex::default(),
                wake: Condvar::new(),
                version: AtomicU64::new(0),
            }),
        }
    }

    pub(crate) fn available(&self) -> Result<(), String> {
        if !cfg!(feature = "people") {
            return Err("Person masks are not in this build.".into());
        }
        let model = ModelTask::Matting.model();
        if !model.is_downloaded() {
            return Err(format!(
                "{} is not downloaded: Settings → Models.",
                model.name
            ));
        }
        Ok(())
    }

    pub(crate) fn version(&self) -> u64 {
        self.shared.version.load(Ordering::SeqCst)
    }

    /// Ask for frames `lo..=hi` of `item` on behalf of `want`, starting
    /// the worker the first time.
    fn request(&self, state: &mut State, want: Want, item: &MediaItem, lo: i64, hi: i64) {
        if self.available().is_err() {
            return;
        }
        state.clock += 1;
        let request = Request {
            item: item.clone(),
            model: ModelTask::Matting.model(),
            lo,
            hi,
            touched: state.clock,
        };
        state.requests.insert(want, request);
        if !state.worker {
            let shared = self.shared.clone();
            let started = std::thread::Builder::new()
                .name("koo-people".into())
                .spawn(move || work(&shared));
            state.worker = started.is_ok();
        }
        self.shared.wake.notify_all();
    }

    pub(crate) fn matte(&self, person: PersonRef<'_>, t: Time) -> Option<PersonMatte> {
        let item = person.item;
        let fp = item.fingerprint.clone()?;
        let model = ModelTask::Matting.model();
        let media = (fp, model.id);
        let (lo, hi) = frames(&person);
        let frame = frame_of(item, t).clamp(lo, hi);
        let mut state = lock(&self.shared.state);
        let ahead = (frame + frames_in(item, AHEAD)).min(hi);
        self.request(&mut state, Want::View(person.mask), item, frame, ahead);
        if let Some(m) = state.matte(&media, frame) {
            return Some(m);
        }
        (1..=frames_in(item, NEAREST)).find_map(|d| {
            [frame - d, frame + d]
                .into_iter()
                .filter(|f| *f >= 0)
                .find_map(|f| state.matte(&media, f))
        })
    }

    pub(crate) fn status(&self, person: PersonRef<'_>) -> SubjectStatus {
        let Some(fp) = person.item.fingerprint.clone() else {
            return SubjectStatus {
                error: Some("The media has no fingerprint yet.".into()),
                ..Default::default()
            };
        };
        let model = ModelTask::Matting.model();
        let media = (fp, model.id);
        let (lo, hi) = frames(&person);
        let last = last_frame(person.item);
        let mut state = lock(&self.shared.state);
        let whole = state.whole(&media).clone();
        let mut made = 0usize;
        for c in lo.div_euclid(CHUNK)..=hi.div_euclid(CHUNK) {
            let start = c * CHUNK;
            let count = if whole.contains(&c) {
                chunk_len(c, last)
            } else {
                state
                    .chunks
                    .get(&(media.0.clone(), media.1, c))
                    .map_or(0, Vec::len)
            } as i64;
            // The part of [start, start + count) within [lo, hi].
            let (a, b) = (start.max(lo), (start + count - 1).min(hi));
            made += (b - a + 1).max(0) as usize;
        }
        let tracking = state.busy.as_ref().is_some_and(|(f, m, c)| {
            *f == media.0
                && *m == media.1
                && (lo.div_euclid(CHUNK)..=hi.div_euclid(CHUNK)).contains(c)
        });
        let error = self.available().err().or_else(|| state.failure(&media));
        SubjectStatus {
            prompted: (0, 0),
            frames: (hi - lo + 1) as usize,
            tracked: made,
            tracking,
            seconds_per_frame: state
                .seconds_per_frame
                .filter(|(id, _)| *id == model.id)
                .map(|(_, s)| s),
            error,
        }
    }

    pub(crate) fn complete(
        &self,
        person: PersonRef<'_>,
        progress: &dyn Fn(f32),
        cancel: &AtomicBool,
    ) -> ServiceResult<()> {
        self.available().map_err(ServiceError)?;
        static RENDERS: AtomicU64 = AtomicU64::new(0);
        let want = Want::Render(RENDERS.fetch_add(1, Ordering::SeqCst));
        let (lo, hi) = frames(&person);
        self.request(&mut lock(&self.shared.state), want, person.item, lo, hi);
        let result = loop {
            let status = self.status(person);
            if let Some(e) = status.error {
                break Err(ServiceError(e));
            }
            if status.tracked >= status.frames {
                progress(1.0);
                break Ok(());
            }
            progress(status.tracked as f32 / status.frames.max(1) as f32);
            if cancel.load(Ordering::SeqCst) {
                break Err(ServiceError("cancelled".into()));
            }
            std::thread::sleep(Duration::from_millis(200));
        };
        lock(&self.shared.state).requests.remove(&want);
        result
    }
}

/// The worker: makes the chunk the newest request wants, one at a time,
/// for good.
fn work(shared: &Shared) {
    let mut matter: Option<(&'static str, Box<dyn Matter>)> = None;
    loop {
        let job = {
            let mut state = lock(&shared.state);
            loop {
                if let Some(job) = state.next() {
                    state.busy = Some(job.key.clone());
                    break job;
                }
                state.busy = None;
                state = shared.wake.wait(state).unwrap_or_else(|e| e.into_inner());
            }
        };
        if matter.as_ref().is_none_or(|(id, _)| *id != job.model.id) {
            matter = runner(job.model).map(|r| (job.model.id, r));
        }
        let result = match &matter {
            Some((_, m)) => make(shared, m.as_ref(), &job),
            None => Err("Person masks are not in this build.".into()),
        };
        let mut state = lock(&shared.state);
        state.busy = None;
        if let Err(e) = result {
            let media = (job.key.0.clone(), job.key.1);
            state.failed.insert(media, (e, model_here()));
        }
        drop(state);
        shared.version.fetch_add(1, Ordering::SeqCst);
    }
}

/// Make the rest of `job`'s chunk, keeping each frame as it comes; stops
/// early (keeping them) when a newer request wants another chunk.
fn make(shared: &Shared, matter: &dyn Matter, job: &Job) -> Result<(), String> {
    let (fp, model, chunk) = job.key.clone();
    let media = (fp.clone(), model);
    let start = chunk * CHUNK + job.made as i64;
    // The frame before, to ease the first one toward.
    let mut previous: Option<PersonMatte> = lock(&shared.state).matte(&media, start - 1);
    let item = &job.item;
    let width = decode_width(item);
    let at = |f: i64| Time::from_frame(f, rate(item));
    let mut stream: Option<Box<dyn FrameStream>> = None;
    for i in job.made..job.len {
        let f = chunk * CHUNK + i as i64;
        let picture = read(shared.media.as_ref(), item, &mut stream, at(f), width)?;
        let begun = Instant::now();
        let alpha = matter.matte(&picture).map_err(|e| format!("{e:#}"))?;
        let mut values = alpha.values;
        if let Some(p) = previous
            .as_ref()
            .filter(|p| p.width == alpha.width && p.height == alpha.height)
        {
            ease(&mut values, &p.alpha);
        }
        let matte = PersonMatte {
            width: alpha.width,
            height: alpha.height,
            alpha: values
                .iter()
                .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
                .collect(),
        };
        let packed = Packed::pack(&matte);
        previous = Some(matte);
        let mut state = lock(&shared.state);
        state.seconds_per_frame = Some((model, begun.elapsed().as_secs_f32()));
        match state.chunks.get_mut(&job.key) {
            Some(frames) if frames.len() == i => frames.push(packed),
            _ => state.keep(job.key.clone(), vec![packed]),
        }
        let whole = i + 1 == job.len;
        if whole {
            let frames = state.chunks.get(&job.key).cloned().unwrap_or_default();
            drop(state);
            if let Err(e) = store::store(&store::chunk_path(&fp, model, chunk), &frames) {
                eprintln!("koo-analysis: {e:#}");
            }
            lock(&shared.state).whole(&media).insert(chunk);
            shared.version.fetch_add(1, Ordering::SeqCst);
            return Ok(());
        }
        let next = state.next().map(|j| j.key);
        drop(state);
        shared.version.fetch_add(1, Ordering::SeqCst);
        if next.as_ref() != Some(&job.key) {
            return Ok(());
        }
    }
    Ok(())
}

/// `values` eased toward the frame before, `previous` (0..255), where they
/// barely differ.
fn ease(values: &mut [f32], previous: &[u8]) {
    for (v, p) in values.iter_mut().zip(previous) {
        let p = *p as f32 / 255.0;
        let keep = 0.5 * (1.0 - (*v - p).abs() / MOTION).clamp(0.0, 1.0);
        *v += (p - *v) * keep;
    }
}

/// The frame of `item` at `t`: the stream's next (opened there the first
/// time), or decoded alone if the stream ends short (variable-rate files
/// can); images are decoded alone.
fn read(
    media: &dyn MediaService,
    item: &MediaItem,
    stream: &mut Option<Box<dyn FrameStream>>,
    t: Time,
    width: u32,
) -> Result<Frame, String> {
    if !is_video(item) {
        return media.frame(item, t, width).map_err(|e| e.0);
    }
    if stream.is_none() {
        *stream = Some(media.playback(item, t, width).map_err(|e| e.0)?);
    }
    let next = stream
        .as_mut()
        .expect("opened above")
        .next_frame()
        .map_err(|e| e.0)?;
    match next {
        Some((_, frame)) => Ok(frame),
        None => media.frame(item, t, width).map_err(|e| e.0),
    }
}
