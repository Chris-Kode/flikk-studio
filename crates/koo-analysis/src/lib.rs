//! Local analysis: bundled Whisper transcription, voice activity, speakers,
//! shot changes, silences and loudness. Implements
//! `koo_core::services::AnalysisService`.
//!
//! [`LocalAnalysis::analyze`] queues a job and returns at once; a small pool
//! of worker threads runs the pipeline (audio → VAD → transcript → speakers →
//! shot changes) and caches the result by fingerprint, so an item is only
//! ever analyzed once (principle 4).

mod cache;
mod ffmpeg;
pub mod speakers;
#[cfg(feature = "whisper")]
mod transcribe;
mod vad;

use koo_core::analysis::{Analysis, Loudness, Transcript};
use koo_core::ids::JobId;
use koo_core::model::{Fingerprint, MediaItem, MediaKind};
use koo_core::services::{AnalysisService, JobState, JobStatus, ServiceError, ServiceResult};
use koo_core::time::{Time, TimeRange};
use speakers::{Diarizer, SingleSpeaker};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard};

pub use ffmpeg::binary as ffmpeg_binary;

/// Engine-wide settings.
#[derive(Clone, Debug)]
pub struct Options {
    /// Items analyzed at once. Whisper already uses several cores per item.
    pub workers: usize,
    /// ffmpeg scene score (0..1) above which a frame starts a new shot.
    pub scene_threshold: f32,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            workers: 2,
            scene_threshold: 0.3,
        }
    }
}

/// Per-item choices for [`LocalAnalysis::analyze_with`].
#[derive(Clone, Debug, Default)]
pub struct AnalyzeOptions {
    /// Audio stream to transcribe, as a position in `MediaInfo::audio`
    /// (0 = first). Defaults to the stream tagged with `language`, else the
    /// first.
    pub audio_stream: Option<u32>,
    /// Spoken language from a source fact (ISO 639-1 or 639-2). Picks the
    /// matching stream and skips Whisper's language detection.
    pub language: Option<String>,
    /// Ignore cached results and analyze again.
    pub refresh: bool,
}

/// The [`AnalysisService`] that runs everything on this machine.
pub struct LocalAnalysis {
    shared: Arc<Shared>,
    queue: Sender<Task>,
}

struct Task {
    job: JobId,
    item: MediaItem,
    fingerprint: Fingerprint,
    options: AnalyzeOptions,
}

struct Shared {
    options: Options,
    #[cfg_attr(not(feature = "whisper"), allow(dead_code))]
    diarizer: Box<dyn Diarizer>,
    jobs: Arc<Mutex<Vec<JobStatus>>>,
    results: Mutex<HashMap<Fingerprint, Arc<Analysis>>>,
    /// Fingerprints queued or running, so the same file is analyzed once.
    in_flight: Mutex<HashMap<Fingerprint, JobId>>,
    /// Results kept in memory although a step failed; analyzed again on
    /// the next request.
    incomplete: Mutex<HashSet<Fingerprint>>,
    #[cfg(feature = "whisper")]
    whisper: transcribe::Whisper,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Default for LocalAnalysis {
    fn default() -> Self {
        Self::new(Options::default())
    }
}

impl LocalAnalysis {
    pub fn new(options: Options) -> Self {
        Self::with_diarizer(options, Box::new(SingleSpeaker))
    }

    /// Use `diarizer` for speaker labels instead of a single "Speaker 1".
    pub fn with_diarizer(options: Options, diarizer: Box<dyn Diarizer>) -> Self {
        let workers = options.workers.max(1);
        let shared = Arc::new(Shared {
            options,
            diarizer,
            jobs: Arc::default(),
            results: Mutex::default(),
            in_flight: Mutex::default(),
            incomplete: Mutex::default(),
            #[cfg(feature = "whisper")]
            whisper: Default::default(),
        });
        let (queue, rx) = channel::<Task>();
        let rx = Arc::new(Mutex::new(rx));
        for i in 0..workers {
            let shared = shared.clone();
            let rx = rx.clone();
            std::thread::Builder::new()
                .name(format!("koo-analysis-{i}"))
                .spawn(move || worker(&shared, &rx))
                .expect("spawn analysis worker");
        }
        Self { shared, queue }
    }

    /// Whether this build can transcribe (the `whisper` feature).
    pub fn transcription_available() -> bool {
        cfg!(feature = "whisper")
    }

    /// Like [`AnalysisService::analyze`], choosing the audio stream and
    /// language, or forcing a fresh run.
    pub fn analyze_with(&self, item: &MediaItem, options: AnalyzeOptions) -> JobId {
        let shared = &self.shared;
        let id = JobId::new();
        let label = format!("Analyze {}", file_name(&item.path));
        let finished = |state| {
            lock(&shared.jobs).push(JobStatus {
                id,
                label: label.clone(),
                media: Some(item.id),
                state,
            });
            id
        };
        let Some(fp) = item.fingerprint.clone() else {
            return finished(JobState::Failed {
                message: format!(
                    "{} has no fingerprint yet; analysis is keyed by it",
                    file_name(&item.path)
                ),
            });
        };
        let mut in_flight = lock(&shared.in_flight);
        if let Some(&running) = in_flight.get(&fp) {
            return running;
        }
        if !options.refresh && shared.complete(&fp) {
            return finished(JobState::Done);
        }
        in_flight.insert(fp.clone(), id);
        drop(in_flight);
        finished(JobState::Queued);
        let task = Task {
            job: id,
            item: item.clone(),
            fingerprint: fp.clone(),
            options,
        };
        if self.queue.send(task).is_err() {
            lock(&shared.in_flight).remove(&fp);
            shared.set_state(
                id,
                JobState::Failed {
                    message: "analysis workers stopped".into(),
                },
            );
        }
        id
    }

    /// Drop finished and failed jobs from [`AnalysisService::jobs`].
    pub fn clear_finished(&self) {
        lock(&self.shared.jobs)
            .retain(|j| matches!(j.state, JobState::Queued | JobState::Running { .. }));
    }
}

impl AnalysisService for LocalAnalysis {
    fn analyze(&self, item: &MediaItem) -> JobId {
        self.analyze_with(item, AnalyzeOptions::default())
    }

    fn jobs(&self) -> Vec<JobStatus> {
        lock(&self.shared.jobs).clone()
    }

    fn analysis(&self, fingerprint: &Fingerprint) -> Option<Arc<Analysis>> {
        self.shared.cached(fingerprint)
    }

    fn silences(&self, item: &MediaItem, min: Time) -> ServiceResult<Vec<TimeRange>> {
        let duration = item.info.as_ref().map(|i| i.duration);
        if let Some(analysis) = item.fingerprint.as_ref().and_then(|fp| self.analysis(fp)) {
            let end = duration.unwrap_or_else(|| analyzed_end(&analysis));
            return Ok(vad::complement(&analysis.speech, end, min));
        }
        if !has_audio(item) {
            return Ok(vad::complement(&[], duration.unwrap_or(Time::ZERO), min));
        }
        let stream = choose_stream(item, &AnalyzeOptions::default());
        match ffmpeg::silences(&item.path, stream, min, duration, &|_| {}) {
            Err(e) if ffmpeg::is_missing_stream(&e) => {
                Ok(vad::complement(&[], duration.unwrap_or(Time::ZERO), min))
            }
            r => r.map_err(ServiceError::new),
        }
    }

    fn loudness(&self, path: &Path, range: Option<TimeRange>) -> ServiceResult<Loudness> {
        ffmpeg::loudness(path, range).map_err(ServiceError::new)
    }
}

impl Shared {
    fn set_state(&self, id: JobId, state: JobState) {
        if let Some(job) = lock(&self.jobs).iter_mut().find(|j| j.id == id) {
            job.state = state;
        }
    }

    /// Whether full results are in memory.
    fn complete(&self, fp: &Fingerprint) -> bool {
        lock(&self.results).contains_key(fp) && !lock(&self.incomplete).contains(fp)
    }

    /// Memory first, then the disk cache.
    fn cached(&self, fp: &Fingerprint) -> Option<Arc<Analysis>> {
        if let Some(a) = lock(&self.results).get(fp) {
            return Some(a.clone());
        }
        let loaded = Arc::new(cache::load(fp)?);
        lock(&self.results).insert(fp.clone(), loaded.clone());
        Some(loaded)
    }
}

fn worker(shared: &Arc<Shared>, rx: &Mutex<Receiver<Task>>) {
    loop {
        let task = match lock(rx).recv() {
            Ok(task) => task,
            Err(_) => return, // the service was dropped
        };
        let state = if !task.options.refresh
            && shared.cached(&task.fingerprint).is_some()
            && shared.complete(&task.fingerprint)
        {
            JobState::Done
        } else {
            shared.set_state(task.job, JobState::Running { fraction: 0.0 });
            run(shared, &task)
        };
        lock(&shared.in_flight).remove(&task.fingerprint);
        shared.set_state(task.job, state);
    }
}

/// Maps each pipeline step onto its share of the job's progress bar.
struct Progress {
    jobs: Arc<Mutex<Vec<JobStatus>>>,
    job: JobId,
    steps: Vec<(Step, f32)>,
}

#[derive(Clone, Copy, PartialEq)]
enum Step {
    Audio,
    Transcript,
    Shots,
}

impl Progress {
    /// Callback for `step` reporting its own 0..1.
    fn step(&self, step: Step) -> impl Fn(f32) + Send + Sync + 'static {
        let total = self
            .steps
            .iter()
            .map(|s| s.1)
            .sum::<f32>()
            .max(f32::EPSILON);
        let at = self.steps.iter().position(|s| s.0 == step);
        let before: f32 = self.steps[..at.unwrap_or(0)].iter().map(|s| s.1).sum();
        let span = at.map_or(0.0, |i| self.steps[i].1);
        let (from, span) = (before / total, span / total);
        let (jobs, job) = (self.jobs.clone(), self.job);
        move |f: f32| {
            if let Some(j) = lock(&jobs).iter_mut().find(|j| j.id == job) {
                j.state = JobState::Running {
                    fraction: (from + span * f.clamp(0.0, 1.0)).min(1.0),
                };
            }
        }
    }
}

/// The whole pipeline for one item. Partial results are kept in memory
/// even when a step fails, but only complete ones are written to disk, so
/// a later session tries again.
fn run(shared: &Shared, task: &Task) -> JobState {
    let item = &task.item;
    let duration = item.info.as_ref().map(|i| i.duration);
    let audio_wanted = has_audio(item);
    let video_wanted = has_video(item);
    let mut steps = Vec::new();
    if audio_wanted {
        steps.push((Step::Audio, 1.0));
        steps.push((Step::Transcript, 6.0));
    }
    if video_wanted {
        steps.push((Step::Shots, 2.0));
    }
    let progress = Progress {
        jobs: shared.jobs.clone(),
        job: task.job,
        steps,
    };

    let mut analysis = Analysis::default();
    let mut problems: Vec<String> = Vec::new();
    // A step failed in a way a rerun could fix.
    let mut incomplete = false;
    let mut unpersisted = false;

    if audio_wanted {
        let stream = choose_stream(item, &task.options);
        let audio = match ffmpeg::extract_audio(
            &item.path,
            stream,
            duration,
            &progress.step(Step::Audio),
        ) {
            Ok(audio) => Some(audio),
            Err(e) if item.info.is_none() && ffmpeg::is_missing_stream(&e) => None,
            Err(e) => {
                problems.push(format!("audio: {e:#}"));
                incomplete = true;
                None
            }
        };
        if let Some(audio) = audio {
            analysis.speech = vad::speech_ranges(&audio);
            match transcript(shared, task, stream, &audio, &analysis.speech, &progress) {
                Ok(Some(t)) => analysis.transcript = Some(t),
                // This build cannot transcribe: keep the rest for this
                // session, but leave the disk cache for a build that can.
                Ok(None) => unpersisted = true,
                Err(e) => {
                    problems.push(format!("transcript: {e:#}"));
                    incomplete = true;
                }
            }
        }
    }

    if video_wanted {
        match ffmpeg::shot_changes(
            &item.path,
            shared.options.scene_threshold,
            duration,
            &progress.step(Step::Shots),
        ) {
            Ok(shots) => analysis.shot_changes = shots,
            Err(e) if item.info.is_none() && ffmpeg::is_missing_stream(&e) => {}
            Err(e) => {
                problems.push(format!("shot changes: {e:#}"));
                incomplete = true;
            }
        }
    }

    let fp = &task.fingerprint;
    if !incomplete
        && !unpersisted
        && let Err(e) = cache::store(fp, &analysis)
    {
        problems.push(format!("cache: {e:#}"));
    }
    lock(&shared.results).insert(fp.clone(), Arc::new(analysis));
    if incomplete {
        lock(&shared.incomplete).insert(fp.clone());
    } else {
        lock(&shared.incomplete).remove(fp);
    }
    if problems.is_empty() {
        JobState::Done
    } else {
        JobState::Failed {
            message: problems.join("; "),
        }
    }
}

/// `Ok(None)` when this build cannot transcribe.
#[cfg(feature = "whisper")]
fn transcript(
    shared: &Shared,
    task: &Task,
    stream: u32,
    audio: &[f32],
    speech: &[TimeRange],
    progress: &Progress,
) -> anyhow::Result<Option<Transcript>> {
    let language = task.options.language.as_deref().and_then(iso_639_1);
    let mut words = Vec::new();
    let mut detected = language.map(str::to_string);
    // Whisper invents words in silence; only run it when someone talks.
    if !speech.is_empty() {
        let download = Mutex::new(None);
        let model = shared.whisper.ensure_model(|name| {
            let id = JobId::new();
            *lock(&download) = Some(id);
            lock(&shared.jobs).push(JobStatus {
                id,
                label: format!("Download {name}"),
                media: None,
                state: JobState::Running { fraction: 0.0 },
            });
            let jobs = shared.jobs.clone();
            Box::new(move |f: f32| {
                if let Some(j) = lock(&jobs).iter_mut().find(|j| j.id == id) {
                    j.state = if f >= 1.0 {
                        JobState::Done
                    } else {
                        JobState::Running { fraction: f }
                    };
                }
            })
        });
        if let (Err(e), Some(id)) = (&model, *lock(&download)) {
            let message = format!("{e:#}");
            shared.set_state(id, JobState::Failed { message });
        }
        let model = model?;
        let out =
            shared
                .whisper
                .transcribe(&model, audio, language, progress.step(Step::Transcript))?;
        words = out.words;
        detected = Some(out.language);
    }
    let speakers = speakers::attribute(&*shared.diarizer, audio, &mut words);
    Ok(Some(Transcript {
        fingerprint: task.fingerprint.clone(),
        audio_stream: stream,
        language: detected.unwrap_or_else(|| "und".into()),
        words,
        speakers,
    }))
}

#[cfg(not(feature = "whisper"))]
fn transcript(
    _shared: &Shared,
    _task: &Task,
    _stream: u32,
    _audio: &[f32],
    _speech: &[TimeRange],
    _progress: &Progress,
) -> anyhow::Result<Option<Transcript>> {
    Ok(None)
}

fn has_audio(item: &MediaItem) -> bool {
    match &item.info {
        Some(info) => !info.audio.is_empty(),
        None => item.kind != MediaKind::Image,
    }
}

fn has_video(item: &MediaItem) -> bool {
    match &item.info {
        Some(info) => info.video.is_some() && item.kind == MediaKind::Video,
        None => item.kind == MediaKind::Video,
    }
}

/// Position in `MediaInfo::audio` to analyze.
fn choose_stream(item: &MediaItem, options: &AnalyzeOptions) -> u32 {
    if let Some(n) = options.audio_stream {
        return n;
    }
    let wanted = options.language.as_deref().and_then(iso_639_1);
    let tagged = item.info.as_ref().zip(wanted).and_then(|(info, wanted)| {
        info.audio.iter().position(|a| {
            a.language
                .as_deref()
                .and_then(iso_639_1)
                .is_some_and(|l| l == wanted)
        })
    });
    tagged.unwrap_or(0) as u32
}

/// Whisper's two-letter code for a 639-1 or common 639-2 tag.
fn iso_639_1(tag: &str) -> Option<&'static str> {
    const CODES: &[(&str, &[&str])] = &[
        ("en", &["eng"]),
        ("es", &["spa"]),
        ("fr", &["fra", "fre"]),
        ("de", &["deu", "ger"]),
        ("it", &["ita"]),
        ("pt", &["por"]),
        ("nl", &["nld", "dut"]),
        ("ru", &["rus"]),
        ("pl", &["pol"]),
        ("tr", &["tur"]),
        ("ar", &["ara"]),
        ("hi", &["hin"]),
        ("ja", &["jpn"]),
        ("ko", &["kor"]),
        ("zh", &["zho", "chi"]),
        ("ca", &["cat"]),
        ("sv", &["swe"]),
        ("uk", &["ukr"]),
    ];
    let tag = tag.trim().to_ascii_lowercase();
    let tag = tag.split(['-', '_']).next().unwrap_or_default();
    if let Some((code, _)) = CODES
        .iter()
        .find(|(code, long)| *code == tag || long.contains(&tag))
    {
        return Some(code);
    }
    #[cfg(feature = "whisper")]
    if tag.len() == 2 && whisper_rs::get_lang_id(tag).is_some() {
        return whisper_rs::get_lang_str(whisper_rs::get_lang_id(tag)?);
    }
    None
}

/// Best guess at the end of an item analyzed without media info.
fn analyzed_end(analysis: &Analysis) -> Time {
    let speech = analysis.speech.iter().map(|r| r.end);
    let words = analysis
        .transcript
        .iter()
        .flat_map(|t| t.words.iter().map(|w| w.range.end));
    speech.chain(words).max().unwrap_or(Time::ZERO)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}
