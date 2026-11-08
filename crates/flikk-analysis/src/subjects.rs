//! Subject masks: [`LocalSubjects`] implements `SubjectService` on the
//! segmentation model (SAM 2.1, `flikk-models`), reading frames through the
//! media engine as the viewer does (the proxy when there is one), numbered
//! on the media's rate.
//!
//! Each prompted frame is segmented by itself and kept by the media's
//! fingerprint, its frame, its prompt and the model, so a frame picked in
//! the viewer is not segmented again once the click lands in a subject. A
//! subject that tracks is followed over every frame its clip shows (and
//! its prompted frames, wherever they are) by one job, its frames kept by
//! fingerprint, [`Subject::key`] and model. Both are kept in memory and
//! under `paths::cache()/mattes/` ([`store`]). Picking another model makes
//! them anew with it.
//!
//! Previews run on a model of their own, so picking stays quick while a
//! subject is tracked, and keep the last frames they encoded for the next
//! clicks. A tracking job waits a moment before it starts, so a few quick
//! clicks run one; a newer subject of the same mask (another click, a trim)
//! cancels the older's job, unless another mask waits on it too. A failed
//! frame or job is tried again after [`RETRY_AFTER`], once the model is
//! downloaded anew, or when a render waits on it.
//!
//! Person masks are its other half ([`crate::people`]): they share its
//! media engine and its version, so views draw again as either makes
//! mattes.
//!
//! [`Subject::key`]: flikk_core::subject::Subject::key

mod store;

use crate::engine::stopped_sentence;
use crate::people::LocalPeople;
use flikk_core::ids::MaskId;
use flikk_core::jobs::{JobProgress, JobQueue};
use flikk_core::model::{Fingerprint, MediaItem, MediaKind};
use flikk_core::person::PersonMatte;
use flikk_core::services::{
    Frame, FrameStream, MediaService, PersonRef, ServiceError, ServiceResult, SubjectRef,
    SubjectService, SubjectStatus,
};
use flikk_core::subject::{Fnv, Matte, SubjectPrompt};
use flikk_core::time::{Rate, Time};
use flikk_models::segmentation::{Encoded, Prompt, Segment, Segmenter};
use flikk_models::{Model, Task as ModelTask};
use flikk_os::sync::lock;
use std::collections::{HashMap, HashSet, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use store::{Frames, Packed};

/// Widest frame read for the model, which sees 1024 × 1024.
const DECODE_WIDTH: u32 = 2048;
/// Frames encoded for previews, kept for the next clicks.
const ENCODED_KEPT: usize = 3;
/// Previews kept in memory besides the subjects' own prompted frames.
const PREVIEWS_KEPT: usize = 256;
/// How long a tracking job waits before it starts.
const SETTLE: Duration = Duration::from_millis(1200);
/// Frames read at once when tracking backward.
const BACKWARD_CHUNK: usize = 24;
/// Tracked frames between the copies kept on disk while tracking.
const SAVE_EVERY: usize = 50;
/// Mattes kept unpacked, for frames drawn again and again.
const UNPACKED_KEPT: usize = 16;
/// Islands and holes smaller than this many of the model's cells are
/// specks: dropped, and filled.
const SPECK: usize = 16;
/// How long a failed frame or job waits before it is tried again.
const RETRY_AFTER: Duration = Duration::from_secs(30);

/// A prompted frame: media, frame, prompt.
type PromptKey = (Fingerprint, i64, u64);
/// A subject followed: media, subject.
type TrackKey = (Fingerprint, u64);
/// A tracking job: its subject and the frames it covers.
type TrackJob = (TrackKey, i64, i64);

/// The [`SubjectService`] that runs the model on this machine.
pub struct LocalSubjects {
    shared: Arc<Shared>,
    /// Prompted frames, one at a time.
    picks: JobQueue<PromptKey>,
    /// Subjects followed, one at a time.
    tracks: JobQueue<TrackJob>,
    /// Person masks.
    people: LocalPeople,
}

struct Shared {
    media: Arc<dyn MediaService>,
    picker: Mutex<Picker>,
    /// The model tracking runs on, loaded on first use.
    tracker: Mutex<Option<(&'static str, Arc<dyn Segmenter>)>>,
    prompted: Mutex<Prompted>,
    tracks: Mutex<HashMap<TrackKey, Frames>>,
    /// Keys already looked for on disk.
    looked: Mutex<HashSet<u64>>,
    /// The tracking jobs queued or running, and the masks waiting on them.
    slots: Mutex<Slots>,
    failed: Mutex<HashMap<u64, Failure>>,
    unpacked: Mutex<VecDeque<(u64, Matte)>>,
    version: AtomicU64,
    /// How long the last tracked frame took, and on which model.
    seconds_per_frame: Mutex<Option<(&'static str, f32)>>,
}

/// Segments single frames, keeping the last few encoded.
struct Picker {
    runner: Option<(&'static str, Box<dyn Segmenter>)>,
    encoded: VecDeque<((Fingerprint, i64), Arc<Encoded>)>,
}

/// Prompted frames made: the subjects' (also on disk) and previews.
#[derive(Default)]
struct Prompted {
    mattes: HashMap<PromptKey, (Packed, bool)>,
    /// Previews, oldest first, to let go of.
    previews: VecDeque<PromptKey>,
}

#[derive(Default)]
struct Slots {
    /// The job each mask waits on, one of `jobs`.
    masks: HashMap<MaskId, TrackJob>,
    /// The jobs queued or running, each with its cancel flag. Masks with
    /// the same subject over the same frames share one.
    jobs: HashMap<TrackJob, Arc<AtomicBool>>,
}

impl Slots {
    /// `mask` waits on no job; its job is cancelled when no other mask
    /// waits on it.
    fn leave(&mut self, mask: MaskId) {
        let Some(job) = self.masks.remove(&mask) else {
            return;
        };
        if !self.masks.values().any(|j| *j == job)
            && let Some(cancel) = self.jobs.remove(&job)
        {
            cancel.store(true, Ordering::SeqCst);
        }
    }
}

/// Frees a tracking job's slot however its work ends: done, failed,
/// panicked, or dropped unrun (the queue had the same job already).
struct Release {
    shared: Arc<Shared>,
    job: TrackJob,
    cancel: Arc<AtomicBool>,
    ran: bool,
}

impl Drop for Release {
    fn drop(&mut self) {
        let mut slots = lock(&self.shared.slots);
        if slots
            .jobs
            .get(&self.job)
            .is_some_and(|c| Arc::ptr_eq(c, &self.cancel))
        {
            slots.jobs.remove(&self.job);
            slots.masks.retain(|_, j| *j != self.job);
        }
        drop(slots);
        // Unrun, it made nothing: the job it met draws again as it ends.
        if self.ran {
            self.shared.bump();
        }
    }
}

/// Why a frame or a job failed.
struct Failure {
    message: String,
    /// Whether the model was there then.
    model: bool,
    at: Instant,
}

fn runner(model: &'static Model) -> Option<Box<dyn Segmenter>> {
    #[cfg(feature = "subjects")]
    {
        Some(flikk_models::segmentation::runner(model))
    }
    #[cfg(not(feature = "subjects"))]
    {
        let _ = model;
        None
    }
}

impl LocalSubjects {
    pub fn new(media: Arc<dyn MediaService>) -> Self {
        Self {
            people: LocalPeople::new(media.clone()),
            shared: Arc::new(Shared {
                media,
                picker: Mutex::new(Picker {
                    runner: None,
                    encoded: VecDeque::new(),
                }),
                tracker: Mutex::default(),
                prompted: Mutex::default(),
                tracks: Mutex::default(),
                looked: Mutex::default(),
                slots: Mutex::default(),
                failed: Mutex::default(),
                unpacked: Mutex::default(),
                version: AtomicU64::new(0),
                seconds_per_frame: Mutex::new(None),
            }),
            picks: JobQueue::new("flikk-subjects-pick", 1),
            tracks: JobQueue::new("flikk-subjects-track", 1),
        }
    }

    /// Whether this build makes subject masks (the `subjects` feature).
    pub fn in_build() -> bool {
        cfg!(feature = "subjects")
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

/// The last frame of `item`, when its length is known.
fn last_frame(item: &MediaItem) -> Option<i64> {
    let duration = item.info.as_ref()?.duration;
    (duration > Time::ZERO).then(|| ((duration - Time(1)).to_frame(rate(item))).max(0))
}

/// What `prompt` asks of `model` on its frame, as a number.
fn prompt_hash(model: &Model, prompt: &SubjectPrompt) -> u64 {
    let mut h = Fnv::default();
    h.write(model.id.as_bytes());
    for p in &prompt.points {
        h.write(&p.at[0].to_bits().to_le_bytes());
        h.write(&p.at[1].to_bits().to_le_bytes());
        h.write(&[p.include as u8]);
    }
    if let Some(r) = prompt.rect {
        for v in r {
            h.write(&v.to_bits().to_le_bytes());
        }
    }
    h.0
}

fn model_prompt(prompt: &SubjectPrompt) -> Prompt {
    Prompt {
        points: prompt.points.iter().map(|p| (p.at, p.include)).collect(),
        rect: prompt.rect,
    }
}

/// The model's answer as kept: without specks.
fn matte(segment: Segment) -> Matte {
    Matte {
        side: segment.side,
        logits: segment.logits.into(),
        score: segment.score,
        present: segment.present,
    }
    .cleaned(SPECK)
}

/// A key of `parts` for maps of keys of every kind.
fn hash(parts: &[&[u8]]) -> u64 {
    let mut h = Fnv::default();
    for p in parts {
        h.write(p);
        h.write(&[0xfe]);
    }
    h.0
}

fn prompt_id(key: &PromptKey) -> u64 {
    hash(&[
        b"p",
        key.0.0.as_bytes(),
        &key.1.to_le_bytes(),
        &key.2.to_le_bytes(),
    ])
}

fn track_id(key: &TrackKey) -> u64 {
    hash(&[b"t", key.0.0.as_bytes(), &key.1.to_le_bytes()])
}

/// What one subject asks of `model`: its prompted frames, and the frames
/// to track.
struct Plan {
    model: &'static Model,
    prompts: Vec<(PromptKey, SubjectPrompt)>,
    /// With tracking: the subject, and the first and last frame.
    track: Option<(TrackKey, i64, i64)>,
}

fn plan(subject: &SubjectRef<'_>) -> Option<Plan> {
    let item = subject.item;
    let fp = item.fingerprint.clone()?;
    let model = ModelTask::Segmentation.model();
    let prompts: Vec<(PromptKey, SubjectPrompt)> = subject
        .subject
        .prompts
        .iter()
        .filter(|p| !p.is_empty())
        .map(|p| {
            let frame = frame_of(item, p.at);
            ((fp.clone(), frame, prompt_hash(model, p)), p.clone())
        })
        .collect();
    let track = (subject.subject.track && is_video(item) && !prompts.is_empty()).then(|| {
        let mut lo = frame_of(item, subject.range.start);
        let mut hi = frame_of(item, subject.range.end - Time(1)).max(lo);
        if let Some(last) = last_frame(item) {
            hi = hi.min(last);
            lo = lo.min(hi);
        }
        for ((_, f, _), _) in &prompts {
            lo = lo.min(*f);
            hi = hi.max(*f);
        }
        let key = hash(&[model.id.as_bytes(), &subject.subject.key().to_le_bytes()]);
        ((fp.clone(), key), lo, hi)
    });
    Some(Plan {
        model,
        prompts,
        track,
    })
}

impl Shared {
    fn bump(&self) {
        self.version.fetch_add(1, Ordering::SeqCst);
    }

    /// The tracker of `model`, made anew when another model was picked.
    fn tracker(&self, model: &'static Model) -> Option<Arc<dyn Segmenter>> {
        let mut tracker = lock(&self.tracker);
        if tracker.as_ref().is_none_or(|(id, _)| *id != model.id) {
            *tracker = runner(model).map(|r| (model.id, Arc::from(r)));
        }
        tracker.as_ref().map(|(_, r)| r.clone())
    }

    fn model_here() -> bool {
        LocalSubjects::in_build() && ModelTask::Segmentation.model().is_downloaded()
    }

    /// Why `id` failed, unless it is to be tried again: the model came
    /// since, or [`RETRY_AFTER`] passed.
    fn failure(&self, id: u64) -> Option<String> {
        let mut failed = lock(&self.failed);
        let f = failed.get(&id)?;
        if (!f.model && Self::model_here()) || f.at.elapsed() >= RETRY_AFTER {
            failed.remove(&id);
            return None;
        }
        Some(f.message.clone())
    }

    fn fail(&self, id: u64, message: String) {
        let failure = Failure {
            message,
            model: Self::model_here(),
            at: Instant::now(),
        };
        lock(&self.failed).insert(id, failure);
        self.bump();
    }

    /// Try `ids` again now, however lately they failed.
    fn retry(&self, ids: impl IntoIterator<Item = u64>) {
        let mut failed = lock(&self.failed);
        for id in ids {
            failed.remove(&id);
        }
    }

    /// The prompted frame `key`, from memory or disk.
    fn prompted(&self, key: &PromptKey) -> Option<Packed> {
        if let Some((p, _)) = lock(&self.prompted).mattes.get(key) {
            return Some(p.clone());
        }
        let id = prompt_id(key);
        if !lock(&self.looked).insert(id) {
            return None;
        }
        let frames = store::load(&store::prompt_path(&key.0, prompt_id(key)))?;
        let packed = frames.frames.into_values().next()?;
        lock(&self.prompted)
            .mattes
            .insert(key.clone(), (packed.clone(), true));
        Some(packed)
    }

    /// Keep the prompted frame `key`: a subject's also on disk, a preview
    /// among the latest.
    fn keep_prompted(&self, key: &PromptKey, packed: Packed, subject: bool) {
        let mut prompted = lock(&self.prompted);
        let stored = prompted.mattes.get(key).is_some_and(|(_, s)| *s);
        if subject && !stored {
            let frames = Frames {
                frames: [(key.1, packed.clone())].into(),
                covered: None,
            };
            if let Err(e) = store::store(&store::prompt_path(&key.0, prompt_id(key)), &frames) {
                eprintln!("flikk-analysis: {e:#}");
            }
        }
        prompted
            .mattes
            .insert(key.clone(), (packed, subject || stored));
        if !subject && !stored {
            prompted.previews.push_back(key.clone());
            while prompted.previews.len() > PREVIEWS_KEPT {
                if let Some(old) = prompted.previews.pop_front()
                    && prompted.mattes.get(&old).is_some_and(|(_, s)| !*s)
                {
                    prompted.mattes.remove(&old);
                }
            }
        }
        drop(prompted);
        // A preview changes nothing drawn until a subject asks for it.
        if subject {
            self.bump();
        }
    }

    /// The tracked frames of `key`, read from disk the first time.
    fn tracked<T>(&self, key: &TrackKey, read: impl FnOnce(Option<&Frames>) -> T) -> T {
        let mut tracks = lock(&self.tracks);
        if !tracks.contains_key(key)
            && lock(&self.looked).insert(track_id(key))
            && let Some(frames) = store::load(&store::track_path(&key.0, key.1))
        {
            tracks.insert(key.clone(), frames);
        }
        read(tracks.get(key))
    }

    /// `item`'s frame `frame` segmented by `prompt` on `model`, encoding it
    /// unless it was one of the last few.
    fn segment(
        &self,
        model: &'static Model,
        item: &MediaItem,
        frame: i64,
        prompt: &Prompt,
    ) -> ServiceResult<Matte> {
        let fp = item
            .fingerprint
            .clone()
            .ok_or_else(|| ServiceError("The media has no fingerprint yet.".into()))?;
        let mut picker = lock(&self.picker);
        let picker = &mut *picker;
        if picker.runner.as_ref().is_none_or(|(id, _)| *id != model.id) {
            picker.runner = runner(model).map(|r| (model.id, r));
            picker.encoded.clear();
        }
        let (_, runner) = picker
            .runner
            .as_ref()
            .ok_or_else(|| ServiceError("Subject masks are not in this build.".into()))?;
        let key = (fp, frame);
        let encoded = match picker.encoded.iter().position(|(k, _)| *k == key) {
            Some(i) => {
                let entry = picker.encoded.remove(i).expect("found above");
                picker.encoded.push_back(entry.clone());
                entry.1
            }
            None => {
                let t = Time::from_frame(frame, rate(item));
                let picture = self.media.frame(item, t, DECODE_WIDTH)?;
                let encoded = Arc::new(runner.encode(&picture).map_err(model_error)?);
                picker.encoded.push_back((key, encoded.clone()));
                while picker.encoded.len() > ENCODED_KEPT {
                    picker.encoded.pop_front();
                }
                encoded
            }
        };
        runner
            .segment(&encoded, prompt)
            .map(matte)
            .map_err(model_error)
    }

    /// The matte `packed` unpacked, from the latest ones when it was
    /// unpacked lately under `id`.
    fn unpack(&self, id: u64, packed: &Packed) -> Option<Matte> {
        let mut unpacked = lock(&self.unpacked);
        if let Some((_, m)) = unpacked.iter().find(|(k, _)| *k == id) {
            return Some(m.clone());
        }
        let m = packed.unpack()?;
        unpacked.push_back((id, m.clone()));
        while unpacked.len() > UNPACKED_KEPT {
            unpacked.pop_front();
        }
        Some(m)
    }
}

fn model_error(e: anyhow::Error) -> ServiceError {
    ServiceError(format!("{e:#}"))
}

impl LocalSubjects {
    /// Queue what `subject` shows and is not made: its prompted frames, and
    /// with tracking the job following it (cancelling the mask's older
    /// one).
    fn ensure(&self, subject: &SubjectRef<'_>, plan: &Plan) {
        if self.available().is_err() {
            return;
        }
        let shared = &self.shared;
        for (key, prompt) in &plan.prompts {
            let id = prompt_id(key);
            if shared.failure(id).is_some() {
                continue;
            }
            if let Some(packed) = shared.prompted(key) {
                // A preview of this prompt: keep it with the subject.
                if !lock(&shared.prompted)
                    .mattes
                    .get(key)
                    .is_some_and(|(_, s)| *s)
                {
                    shared.keep_prompted(key, packed, true);
                }
                continue;
            }
            let (s, model, item, key, prompt) = (
                shared.clone(),
                plan.model,
                subject.item.clone(),
                key.clone(),
                model_prompt(prompt),
            );
            self.picks.clear_finished();
            let job = self
                .picks
                .push(key.clone(), "Find subject", Some(item.id), move |_| {
                    let made =
                        catch_unwind(AssertUnwindSafe(|| s.segment(model, &item, key.1, &prompt)))
                            .unwrap_or_else(|p| {
                                Err(ServiceError(stopped_sentence("Finding the subject", &*p)))
                            });
                    match made {
                        Ok(m) => {
                            s.keep_prompted(&key, Packed::pack(&m), true);
                            Ok(())
                        }
                        Err(e) => {
                            s.fail(prompt_id(&key), e.0.clone());
                            Err(e.0)
                        }
                    }
                });
            // Failed unrun (no thread could start): kept, or it is asked
            // for again on every frame.
            if let Some(message) = self.picks.failed(job) {
                shared.fail(id, message);
            }
        }
        let Some(job) = plan.track.clone() else {
            return;
        };
        let (key, lo, hi) = job.clone();
        if shared.failure(track_id(&key)).is_some() {
            return;
        }
        let covered = shared.tracked(&key, |f| {
            f.and_then(|f| f.covered)
                .is_some_and(|(a, b)| a <= lo && b >= hi)
        });
        let mut slots = lock(&shared.slots);
        if covered {
            if slots.masks.get(&subject.mask).is_some_and(|j| j.0 != key) {
                slots.leave(subject.mask);
            }
            return;
        }
        if slots.masks.get(&subject.mask) != Some(&job) {
            slots.leave(subject.mask);
            slots.masks.insert(subject.mask, job.clone());
        }
        if slots.jobs.contains_key(&job) {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        slots.jobs.insert(job.clone(), cancel.clone());
        // Unlocked first: a job the queue has already drops its work, and
        // with it the release, at once.
        drop(slots);
        let prompts: Vec<(i64, Prompt)> = plan
            .prompts
            .iter()
            .map(|((_, f, _), p)| (*f, model_prompt(p)))
            .collect();
        let (model, item) = (plan.model, subject.item.clone());
        let mut release = Release {
            shared: shared.clone(),
            job: job.clone(),
            cancel,
            ran: false,
        };
        self.tracks.clear_finished();
        let id = self.tracks.push(
            job.clone(),
            "Track subject",
            Some(item.id),
            move |progress| {
                release.ran = true;
                let (s, cancel) = (&release.shared, &release.cancel);
                catch_unwind(AssertUnwindSafe(|| {
                    track(s, model, &item, &job, &prompts, cancel, progress)
                }))
                .unwrap_or_else(|p| {
                    if cancel.load(Ordering::SeqCst) {
                        return Ok(());
                    }
                    let message = stopped_sentence("Tracking", &*p);
                    s.fail(track_id(&job.0), message.clone());
                    Err(message)
                })
            },
        );
        // Failed unrun (no thread could start): kept, as above.
        if let Some(message) = self.tracks.failed(id) {
            shared.fail(track_id(&key), message);
        }
    }
}

/// Follow the subject of `job` over its frames on `model`, keeping each as
/// it comes.
fn track(
    s: &Shared,
    model: &'static Model,
    item: &MediaItem,
    job: &TrackJob,
    prompts: &[(i64, Prompt)],
    cancel: &AtomicBool,
    progress: &JobProgress,
) -> Result<(), String> {
    let start = Instant::now();
    while start.elapsed() < SETTLE {
        if cancel.load(Ordering::SeqCst) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let Some(tracker) = s.tracker(model) else {
        return Err("Subject masks are not in this build.".into());
    };
    let (key, lo, hi) = job;
    let count = (hi - lo + 1) as usize;
    let at: Vec<(usize, Prompt)> = prompts
        .iter()
        .map(|(f, p)| ((f - lo) as usize, p.clone()))
        .collect();
    let mut reader = Reader {
        media: s.media.as_ref(),
        item,
        rate: rate(item),
        first: *lo,
        stream: None,
        chunk: None,
        last: None,
    };
    let (mut made, mut last) = (0usize, Instant::now());
    let result = tracker.track(count, &at, &mut |i| reader.get(i), &mut |i, segment| {
        if cancel.load(Ordering::SeqCst) {
            anyhow::bail!("cancelled");
        }
        let packed = Packed::pack(&matte(segment));
        let mut tracks = lock(&s.tracks);
        let frames = tracks.entry(key.clone()).or_default();
        frames.frames.insert(lo + i as i64, packed);
        made += 1;
        if made.is_multiple_of(SAVE_EVERY) {
            let _ = store::store(&store::track_path(&key.0, key.1), frames);
        }
        drop(tracks);
        *lock(&s.seconds_per_frame) = Some((model.id, last.elapsed().as_secs_f32()));
        last = Instant::now();
        progress.set(made as f32 / count as f32);
        s.bump();
        Ok(())
    });
    match result {
        Ok(()) => {
            let mut tracks = lock(&s.tracks);
            let frames = tracks.entry(key.clone()).or_default();
            frames.covered = Some((*lo, *hi));
            if let Err(e) = store::store(&store::track_path(&key.0, key.1), frames) {
                eprintln!("flikk-analysis: {e:#}");
            }
            Ok(())
        }
        Err(_) if cancel.load(Ordering::SeqCst) => Ok(()),
        Err(e) => {
            let message = format!("{e:#}");
            s.fail(track_id(key), message.clone());
            Err(message)
        }
    }
}

/// Frames of `item` from frame `first` on, read in the order the tracker
/// asks: a stream going forward, chunks read ahead going backward.
struct Reader<'a> {
    media: &'a dyn MediaService,
    item: &'a MediaItem,
    rate: Rate,
    first: i64,
    /// The index it yields next.
    stream: Option<(usize, Box<dyn FrameStream>)>,
    chunk: Option<(usize, Vec<Frame>)>,
    last: Option<usize>,
}

impl Reader<'_> {
    fn get(&mut self, i: usize) -> anyhow::Result<Frame> {
        let backward = self.last == Some(i + 1);
        self.last = Some(i);
        if let Some((start, frames)) = &self.chunk
            && (*start..start + frames.len()).contains(&i)
        {
            return Ok(frames[i - start].clone());
        }
        if backward {
            let start = (i + 1).saturating_sub(BACKWARD_CHUNK);
            let mut stream = self.open(start)?;
            let mut frames = Vec::with_capacity(i + 1 - start);
            for k in start..=i {
                frames.push(self.next(&mut stream, k)?);
            }
            let frame = frames[i - start].clone();
            self.chunk = Some((start, frames));
            return Ok(frame);
        }
        if self.stream.as_ref().is_none_or(|(next, _)| *next != i) {
            self.stream = Some((i, self.open(i)?));
        }
        let (next, mut stream) = self.stream.take().expect("opened above");
        let frame = self.next(&mut stream, i)?;
        self.stream = Some((next + 1, stream));
        Ok(frame)
    }

    fn time(&self, i: usize) -> Time {
        Time::from_frame(self.first + i as i64, self.rate)
    }

    fn open(&self, i: usize) -> anyhow::Result<Box<dyn FrameStream>> {
        self.media
            .playback(self.item, self.time(i), DECODE_WIDTH)
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    /// The stream's next frame, frame `i`; decoded alone if the stream ends
    /// short (variable-rate files can).
    fn next(&self, stream: &mut Box<dyn FrameStream>, i: usize) -> anyhow::Result<Frame> {
        match stream.next_frame().map_err(|e| anyhow::anyhow!("{e}"))? {
            Some((_, frame)) => Ok(frame),
            None => self
                .media
                .frame(self.item, self.time(i), DECODE_WIDTH)
                .map_err(|e| anyhow::anyhow!("{e}")),
        }
    }
}

impl SubjectService for LocalSubjects {
    fn available(&self) -> Result<(), String> {
        if !Self::in_build() {
            return Err("Subject masks are not in this build.".into());
        }
        let model = ModelTask::Segmentation.model();
        if !model.is_downloaded() {
            return Err("Download it in Settings → Integrations.".into());
        }
        Ok(())
    }

    fn matte(&self, subject: SubjectRef<'_>, t: Time) -> Option<Matte> {
        let plan = plan(&subject)?;
        self.ensure(&subject, &plan);
        let shared = &self.shared;
        let frame = frame_of(subject.item, t);
        let mut best: Option<(i64, u64, Packed)> = None;
        let mut consider = |f: i64, id: u64, p: Packed| {
            if best
                .as_ref()
                .is_none_or(|(b, _, _)| f.abs_diff(frame) < b.abs_diff(frame))
            {
                best = Some((f, id, p));
            }
        };
        if let Some((key, _, _)) = &plan.track {
            shared.tracked(key, |frames| {
                let Some(frames) = frames else { return };
                let around = [
                    frames.frames.range(..=frame).next_back(),
                    frames.frames.range(frame..).next(),
                ];
                for (f, p) in around.into_iter().flatten() {
                    consider(
                        *f,
                        hash(&[&track_id(key).to_le_bytes(), &f.to_le_bytes()]),
                        p.clone(),
                    );
                }
            });
        }
        for (key, _) in &plan.prompts {
            if let Some(p) = shared.prompted(key) {
                consider(key.1, prompt_id(key), p);
            }
        }
        let (_, id, packed) = best?;
        shared.unpack(id, &packed)
    }

    fn status(&self, subject: SubjectRef<'_>) -> SubjectStatus {
        let Some(plan) = plan(&subject) else {
            return SubjectStatus {
                error: Some("The media has no fingerprint yet.".into()),
                ..Default::default()
            };
        };
        let shared = &self.shared;
        let made = plan
            .prompts
            .iter()
            .filter(|(k, _)| shared.prompted(k).is_some())
            .count();
        let mut error = plan
            .prompts
            .iter()
            .find_map(|(k, _)| shared.failure(prompt_id(k)));
        let (mut frames, mut tracked, mut tracking) = (1, 0, false);
        if let Some((key, lo, hi)) = &plan.track {
            frames = (hi - lo + 1) as usize;
            tracked = shared.tracked(key, |f| f.map_or(0, |f| f.frames.range(*lo..=*hi).count()));
            tracking = {
                let slots = lock(&shared.slots);
                slots
                    .masks
                    .get(&subject.mask)
                    .is_some_and(|j| j.0 == *key && slots.jobs.contains_key(j))
            };
            error = error.or_else(|| shared.failure(track_id(key)));
        }
        SubjectStatus {
            prompted: (made, plan.prompts.len()),
            frames,
            tracked,
            tracking,
            seconds_per_frame: lock(&shared.seconds_per_frame)
                .filter(|(id, _)| *id == plan.model.id)
                .map(|(_, s)| s),
            error,
        }
    }

    fn complete(
        &self,
        subject: SubjectRef<'_>,
        progress: &dyn Fn(f32),
        cancel: &AtomicBool,
    ) -> ServiceResult<()> {
        self.available().map_err(ServiceError)?;
        let track = subject.subject.track && is_video(subject.item);
        // A render tries what failed once more, however lately.
        if let Some(plan) = plan(&subject) {
            let prompts = plan.prompts.iter().map(|(k, _)| prompt_id(k));
            let tracked = plan.track.iter().map(|(k, _, _)| track_id(k));
            self.shared.retry(prompts.chain(tracked));
        }
        loop {
            if let Some(plan) = plan(&subject) {
                self.ensure(&subject, &plan);
            }
            let status = self.status(subject);
            if let Some(e) = status.error {
                return Err(ServiceError(e));
            }
            if status.complete(track) {
                progress(1.0);
                return Ok(());
            }
            let (made, all) = status.prompted;
            let done = if track {
                status.tracked as f32 / status.frames.max(1) as f32
            } else {
                made as f32 / all.max(1) as f32
            };
            progress(done);
            if cancel.load(Ordering::SeqCst) {
                return Err(ServiceError("cancelled".into()));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    fn preview(&self, item: &MediaItem, prompt: &SubjectPrompt) -> ServiceResult<Matte> {
        self.available().map_err(ServiceError)?;
        let fp = item
            .fingerprint
            .clone()
            .ok_or_else(|| ServiceError("The media has no fingerprint yet.".into()))?;
        let model = ModelTask::Segmentation.model();
        let frame = frame_of(item, prompt.at);
        let key = (fp, frame, prompt_hash(model, prompt));
        if let Some(packed) = self.shared.prompted(&key) {
            return self
                .shared
                .unpack(prompt_id(&key), &packed)
                .ok_or_else(|| ServiceError("A kept matte could not be read.".into()));
        }
        let m = self
            .shared
            .segment(model, item, frame, &model_prompt(prompt))?;
        self.shared.keep_prompted(&key, Packed::pack(&m), false);
        Ok(m)
    }

    fn version(&self) -> u64 {
        self.shared.version.load(Ordering::SeqCst) + self.people.version()
    }

    fn people_available(&self) -> Result<(), String> {
        self.people.available()
    }

    fn person(&self, person: PersonRef<'_>, t: Time) -> Option<PersonMatte> {
        self.people.matte(person, t)
    }

    fn person_status(&self, person: PersonRef<'_>) -> SubjectStatus {
        self.people.status(person)
    }

    fn complete_person(
        &self,
        person: PersonRef<'_>,
        progress: &dyn Fn(f32),
        cancel: &AtomicBool,
    ) -> ServiceResult<()> {
        self.people.complete(person, progress, cancel)
    }
}
