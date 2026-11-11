//! Local analysis: bundled Whisper transcription, voice activity, speakers,
//! sounds, music and beats, sound vectors, shot changes, silences and
//! loudness. Implements `flikk_core::services::AnalysisService`.
//!
//! [`LocalAnalysis::analyze`] queues a job and returns at once; the jobs run
//! one at a time, in the order queued, on a [`JobQueue`]. Each runs the
//! pipeline (audio → the speech Silero hears, or the energy VAD's without
//! it, and levels → transcript of that speech → word times from the
//! aligner, or word edges on the audio where it can't → the speech no word
//! was heard in, heard again → speakers → sounds, music and sound vectors
//! → beats → shot changes) and caches the result by fingerprint with the
//! steps it covers, so an item is analyzed once (principle 4) until the
//! user analyzes it again. A request for steps the result lacks runs only
//! those parts (speech, the sound pass, shot changes) and adds them to it.
//! Results of another [`ANALYSIS_VERSION`] are not read.
//!
//! [`AnalysisService::cancel`] stops an item's jobs: one queued never runs,
//! and one running stops at its next ffmpeg progress line or read, or
//! before its next model step, keeping nothing (neither in memory nor on
//! disk, nor as a failure), as if it had not been asked for.
//!
//! With a provider set for transcription ([`AnalysisConfig::speech`]) the
//! speech steps are its: the item's audio goes to it as lossless FLAC and
//! comes back as words with their times and voices, kept as it aligned
//! them; speech is where the words are. No local speech model runs, and
//! the provider's audio events stand in for the sound tagger when its
//! model is missing.
//!
//! Every step runs whenever its model is downloaded; one whose model is
//! missing is left out (voice activity falls back to an energy VAD).
//!
//! Library sounds are analyzed without being asked, so
//! [`AnalyzeOptions::sound_only`] leaves speech and shots out: nothing goes
//! to a provider, and analyzing the item adds the rest.
//!
//! A voiceover's words are its script ([`GenerationInfo::script`]), so
//! nothing transcribes it and nothing goes to a provider: the aligner
//! follows the script through the audio. It is analyzed as soon as it is
//! made, so its models are optional too.

mod align;
mod cache;
mod cleanup;
mod depth;
mod engine;
mod ffmpeg;
mod interpolation;
mod motion;
mod people;
mod script;
#[cfg(feature = "sounds")]
mod sounds;
pub mod speakers;
mod subjects;
mod vad;

#[cfg(feature = "aligner")]
use flikk_core::analysis::Levels;
use flikk_core::analysis::{
    ANALYSIS_VERSION, Analysis, AnalysisSteps, Loudness, Music, Sound, SoundVectors, Transcript,
    Word,
};
use flikk_core::cloud::{self, GenerationInfo, SpeechRequest, SpeechService};
use flikk_core::ids::JobId;
use flikk_core::jobs::{JobProgress, JobQueue};
use flikk_core::model::{Fingerprint, MediaItem, MediaKind};
use flikk_core::services::{
    AnalysisConfig, AnalysisService, Coverage, JobStatus, ServiceError, ServiceResult,
};
#[cfg(feature = "aligner")]
use flikk_core::time::FLICKS_PER_SECOND;
use flikk_core::time::{Time, TimeRange};
use flikk_models::Task as ModelTask;
use flikk_os::sync::lock;
use speakers::{Diarizer, SingleSpeaker};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub use cleanup::LocalCleanup;
pub use depth::LocalDepth;
pub use ffmpeg::cut_speech;
pub use interpolation::{LocalInterpolation, clear_disk as clear_interpolated};
pub use motion::LocalStabilizer;
pub use subjects::LocalSubjects;

/// Items analyzed at once. Each already keeps several cores busy, and the
/// large models a lot of memory.
const JOBS_AT_ONCE: usize = 1;

/// What a cancelled job returns; it is not listed.
const CANCELLED: &str = "cancelled";

/// Highest sample rate of the audio a provider hears; above it, the audio
/// is resampled. Speech has nothing up there.
const UPLOAD_MAX_RATE: u32 = 48_000;

/// Engine-wide settings.
#[derive(Clone, Debug)]
pub struct Options {
    /// ffmpeg scene score (0..1) above which a frame starts a new shot.
    pub scene_threshold: f32,
}

impl Default for Options {
    fn default() -> Self {
        Self {
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
    /// Only what is heard: no transcript, speakers or shot changes.
    pub sound_only: bool,
}

/// The [`AnalysisService`] that runs everything on this machine.
pub struct LocalAnalysis {
    shared: Arc<Shared>,
    queue: JobQueue<Key>,
}

/// A job's key: the item's fingerprint, the steps asked for and whether
/// the results are replaced. A request for more steps, or to analyze
/// again, waits behind a job for fewer instead of being taken for it, and
/// adds what that job left out. Jobs run one at a time ([`JOBS_AT_ONCE`]),
/// so a file is analyzed once at a time.
type Key = (Fingerprint, AnalysisSteps, bool);

struct Task {
    item: MediaItem,
    fingerprint: Fingerprint,
    options: AnalyzeOptions,
}

struct Shared {
    options: Options,
    diarizer: Box<dyn Diarizer>,
    /// Results read or made so far, with the steps each covers. A step that
    /// failed is not among them, so the next request runs it again; only
    /// results without failures are written to disk.
    results: Mutex<HashMap<Fingerprint, Cached>>,
    /// Fingerprints with no result on disk the last time it was read, so
    /// that asking again reads nothing. A job's result goes to `results`,
    /// which is asked first.
    absent: Mutex<HashSet<Fingerprint>>,
    /// The steps a plain request asks for, without a script and with one,
    /// and when they were worked out: it looks for the models on disk.
    wanted: Mutex<[Option<(Instant, AnalysisSteps)>; 2]>,
    #[cfg(feature = "whisper")]
    voice: Box<dyn flikk_models::voice_activity::VoiceActivity>,
    #[cfg(feature = "whisper")]
    transcriber: Picked<dyn flikk_models::transcription::Transcriber>,
    #[cfg(feature = "aligner")]
    aligner: Box<dyn flikk_models::word_timing::Aligner>,
    #[cfg(feature = "sounds")]
    sounds: sounds::SoundModels,
    /// Sound vectors read or made so far, by fingerprint; `None` when the
    /// sound pass of the result in memory made none, so that older ones on
    /// disk are not read.
    vectors: Mutex<HashMap<Fingerprint, Option<Arc<SoundVectors>>>>,
    /// The steps, and who transcribes, for jobs that start from now on.
    config: Mutex<AnalysisConfig>,
}

/// A result and the steps it covers.
#[derive(Clone)]
struct Cached {
    analysis: Arc<Analysis>,
    steps: AnalysisSteps,
}

/// The runner of the model a task runs on, made again when the user picks
/// another. A job keeps the runner it started with.
#[cfg(feature = "whisper")]
struct Picked<R: ?Sized> {
    task: ModelTask,
    make: fn(&flikk_models::Model) -> Box<R>,
    runner: Mutex<(&'static str, Arc<R>)>,
}

#[cfg(feature = "whisper")]
impl<R: ?Sized> Picked<R> {
    fn new(task: ModelTask, make: fn(&flikk_models::Model) -> Box<R>) -> Self {
        let model = task.model();
        Self {
            task,
            make,
            runner: Mutex::new((model.id, make(model).into())),
        }
    }

    fn get(&self) -> Arc<R> {
        let model = self.task.model();
        let mut runner = lock(&self.runner);
        if runner.0 != model.id {
            *runner = (model.id, (self.make)(model).into());
        }
        runner.1.clone()
    }
}

impl Default for LocalAnalysis {
    fn default() -> Self {
        Self::new(Options::default())
    }
}

impl LocalAnalysis {
    pub fn new(options: Options) -> Self {
        #[cfg(feature = "diarizer")]
        let diarizer = flikk_models::speakers::runner(ModelTask::Speakers.model());
        #[cfg(not(feature = "diarizer"))]
        let diarizer = Box::new(SingleSpeaker);
        Self::with_diarizer(options, diarizer)
    }

    /// Use `diarizer` for speaker labels instead of the selected speakers
    /// model. The other tasks run on their selected models.
    pub fn with_diarizer(options: Options, diarizer: Box<dyn Diarizer>) -> Self {
        let shared = Arc::new(Shared {
            options,
            diarizer,
            results: Mutex::default(),
            absent: Mutex::default(),
            wanted: Mutex::default(),
            #[cfg(feature = "whisper")]
            voice: flikk_models::voice_activity::runner(ModelTask::VoiceActivity.model()),
            #[cfg(feature = "whisper")]
            transcriber: Picked::new(
                ModelTask::Transcription,
                flikk_models::transcription::runner,
            ),
            #[cfg(feature = "aligner")]
            aligner: flikk_models::word_timing::runner(ModelTask::WordTiming.model()),
            #[cfg(feature = "sounds")]
            sounds: Default::default(),
            vectors: Mutex::default(),
            config: Mutex::default(),
        });
        Self {
            shared,
            queue: JobQueue::new("flikk-analysis", JOBS_AT_ONCE),
        }
    }

    /// Whether this build can transcribe (the `whisper` feature).
    pub fn transcription_available() -> bool {
        cfg!(feature = "whisper")
    }

    /// The tasks this build runs on a model, in the order it runs them.
    /// Each model is downloaded only when the user asks; a step whose model
    /// is missing fails, and the result is kept for the session only.
    pub fn model_tasks() -> Vec<ModelTask> {
        let mut tasks = Vec::new();
        if cfg!(feature = "whisper") {
            tasks.extend([ModelTask::VoiceActivity, ModelTask::Transcription]);
        }
        if cfg!(feature = "aligner") {
            tasks.push(ModelTask::WordTiming);
        }
        if cfg!(feature = "diarizer") {
            tasks.push(ModelTask::Speakers);
        }
        if cfg!(feature = "sounds") {
            tasks.extend([
                ModelTask::SoundTagging,
                ModelTask::Beats,
                ModelTask::SoundSearch,
            ]);
        }
        if cfg!(feature = "subjects") {
            tasks.push(ModelTask::Segmentation);
        }
        if cfg!(feature = "interpolation") {
            tasks.push(ModelTask::FrameInterpolation);
        }
        if cfg!(feature = "people") {
            tasks.push(ModelTask::Matting);
        }
        if cfg!(feature = "depth") {
            tasks.push(ModelTask::Depth);
        }
        tasks
    }

    /// The tasks whose models must be downloaded before an item can be
    /// analyzed with `steps`. When a provider transcribes (`online`), the
    /// ones it does ([`Self::online_tasks`]) are not among them.
    pub fn required_tasks(steps: &AnalysisSteps, online: bool) -> Vec<ModelTask> {
        Self::model_tasks()
            .into_iter()
            .filter(|task| !(online && Self::online_tasks().contains(task)))
            .filter(|task| match task {
                // Voice activity runs with every item that has audio.
                ModelTask::VoiceActivity => true,
                ModelTask::Transcription | ModelTask::WordTiming => steps.transcript,
                ModelTask::Speakers => steps.speakers(),
                ModelTask::SoundTagging => steps.sounds,
                ModelTask::Beats => steps.beats(),
                ModelTask::SoundSearch => steps.sound_search(),
                // Subject masks, not analysis.
                ModelTask::Segmentation => false,
                // Optical flow slow motion, not analysis.
                ModelTask::FrameInterpolation => false,
                // Person masks, not analysis.
                ModelTask::Matting => false,
                // Depth maps, not analysis.
                ModelTask::Depth => false,
            })
            .collect()
    }

    /// The tasks a provider does in place of a local model when it
    /// transcribes.
    pub fn online_tasks() -> [ModelTask; 4] {
        [
            ModelTask::VoiceActivity,
            ModelTask::Transcription,
            ModelTask::WordTiming,
            ModelTask::Speakers,
        ]
    }

    /// Like [`AnalysisService::analyze`], choosing the audio stream and
    /// language, or forcing a fresh run.
    pub fn analyze_with(&self, item: &MediaItem, options: AnalyzeOptions) -> JobId {
        let label = format!("Analyze {}", file_name(&item.path));
        let Some(fp) = item.fingerprint.clone() else {
            let message = format!(
                "{} has no fingerprint yet; analysis is keyed by it",
                file_name(&item.path)
            );
            return self.queue.finished(label, Some(item.id), Err(message));
        };
        let wanted = Plan::for_item(&self.shared, item, options.sound_only).ran();
        if !options.refresh && self.shared.covers(&fp, &wanted) {
            return self.queue.finished(label, Some(item.id), Ok(()));
        }
        let shared = self.shared.clone();
        let key = (fp.clone(), wanted, options.refresh);
        let task = Task {
            item: item.clone(),
            fingerprint: fp,
            options,
        };
        self.queue.push(key, label, Some(item.id), move |job| {
            run(&shared, &task, job)
        })
    }

    /// Drop finished and failed jobs from [`AnalysisService::jobs`].
    pub fn clear_finished(&self) {
        self.queue.clear_finished();
    }
}

impl AnalysisService for LocalAnalysis {
    fn analyze(&self, item: &MediaItem) -> JobId {
        self.analyze_with(item, AnalyzeOptions::default())
    }

    fn analyze_sound(&self, item: &MediaItem) {
        let wanted = cfg!(feature = "sounds")
            && lock(&self.shared.config).steps.sounds
            && ModelTask::SoundTagging.model().is_downloaded();
        let heard = || Plan::for_item(&self.shared, item, true).ran();
        let done = |fp| self.shared.covers(fp, &heard());
        if wanted && item.fingerprint.as_ref().is_some_and(|fp| !done(fp)) {
            let options = AnalyzeOptions {
                sound_only: true,
                ..AnalyzeOptions::default()
            };
            self.analyze_with(item, options);
        }
    }

    fn analyze_again(&self, item: &MediaItem) -> JobId {
        let options = AnalyzeOptions {
            refresh: true,
            ..AnalyzeOptions::default()
        };
        self.analyze_with(item, options)
    }

    fn configure(&self, config: AnalysisConfig) {
        *lock(&self.shared.config) = config;
        *lock(&self.shared.wanted) = [None, None];
    }

    fn cancel(&self, item: &MediaItem) {
        // Every job for its file: each set of steps, again or not.
        if let Some(fp) = &item.fingerprint {
            self.queue.cancel_where(|(f, _, _)| f == fp);
        }
    }

    fn jobs(&self) -> Vec<JobStatus> {
        self.queue.jobs()
    }

    fn analysis(&self, fingerprint: &Fingerprint) -> Option<Arc<Analysis>> {
        self.shared.cached(fingerprint)
    }

    fn coverage(&self, item: &MediaItem) -> Coverage {
        let Some(cached) = item
            .fingerprint
            .as_ref()
            .and_then(|fp| self.shared.entry(fp))
        else {
            return Coverage::None;
        };
        match cached.steps.covers(&self.shared.wanted(has_script(item))) {
            true => Coverage::Full,
            false => Coverage::Partial,
        }
    }

    fn time_script(&self, path: &Path, script: &str) -> ServiceResult<Vec<Word>> {
        script::time(&self.shared, path, script)
    }

    fn silences(&self, item: &MediaItem, min: Time) -> ServiceResult<Vec<TimeRange>> {
        let duration = item.info.as_ref().map(|i| i.duration);
        if let Some(analysis) = item.fingerprint.as_ref().and_then(|fp| self.analysis(fp)) {
            let end = duration.unwrap_or_else(|| analyzed_end(&analysis));
            return Ok(vad::complement(&talking(&analysis), end, min));
        }
        if !has_audio(item) {
            return Ok(vad::complement(&[], duration.unwrap_or(Time::ZERO), min));
        }
        let stream = choose_stream(&self.shared, item, &AnalyzeOptions::default());
        match ffmpeg::silences(&item.path, stream, min, duration, &|_| {}, &|| false) {
            Err(e) if ffmpeg::is_missing_stream(&e) => {
                Ok(vad::complement(&[], duration.unwrap_or(Time::ZERO), min))
            }
            r => r.map_err(ServiceError::new),
        }
    }

    fn loudness(&self, path: &Path, range: Option<TimeRange>) -> ServiceResult<Loudness> {
        ffmpeg::loudness(path, range).map_err(ServiceError::new)
    }

    fn sound_vectors(&self, fingerprint: &Fingerprint) -> Option<Arc<SoundVectors>> {
        let shared = &self.shared;
        if let Some(v) = lock(&shared.vectors).get(fingerprint) {
            return v.clone();
        }
        // Vectors beside a result of another version, or one that does not
        // cover sound search, are not read either.
        if !shared.entry(fingerprint)?.steps.sound_search() {
            return None;
        }
        let loaded = Arc::new(cache::load_vectors(fingerprint)?);
        lock(&shared.vectors).insert(fingerprint.clone(), Some(loaded.clone()));
        Some(loaded)
    }

    #[cfg(feature = "sounds")]
    fn describe_sound(&self, text: &str) -> ServiceResult<Vec<f32>> {
        let shared = &self.shared;
        shared
            .sounds
            .describe(text)
            .map_err(|e| ServiceError(format!("{e:#}")))
    }
}

impl Shared {
    /// Whether results in memory or on disk hold everything a run of
    /// `wanted` makes.
    fn covers(&self, fp: &Fingerprint, wanted: &AnalysisSteps) -> bool {
        self.entry(fp).is_some_and(|c| c.steps.covers(wanted))
    }

    fn cached(&self, fp: &Fingerprint) -> Option<Arc<Analysis>> {
        self.entry(fp).map(|c| c.analysis)
    }

    /// What [`Plan::for_item`] runs for an item that is not sound only,
    /// worked out again after [`WANTED_FOR`], as models come and go.
    fn wanted(&self, script: bool) -> AnalysisSteps {
        let now = Instant::now();
        let fresh = |(at, _): &(Instant, AnalysisSteps)| now.duration_since(*at) < WANTED_FOR;
        if let Some((_, steps)) = lock(&self.wanted)[script as usize].filter(fresh) {
            return steps;
        }
        let steps = Plan::new(&lock(&self.config), false, script).ran();
        lock(&self.wanted)[script as usize] = Some((now, steps));
        steps
    }

    /// Memory first, then the disk cache, once.
    fn entry(&self, fp: &Fingerprint) -> Option<Cached> {
        if let Some(c) = lock(&self.results).get(fp) {
            return Some(c.clone());
        }
        if lock(&self.absent).contains(fp) {
            return None;
        }
        let Some((analysis, steps)) = cache::load(fp) else {
            lock(&self.absent).insert(fp.clone());
            return None;
        };
        let loaded = Cached {
            analysis: Arc::new(analysis),
            steps,
        };
        // A job may have finished while the disk was read.
        let mut results = lock(&self.results);
        Some(results.entry(fp.clone()).or_insert(loaded).clone())
    }
}

/// How long [`Shared::wanted`] holds the steps it worked out.
const WANTED_FOR: Duration = Duration::from_secs(2);

/// A voiceover, whose script stands in for transcription.
fn has_script(item: &MediaItem) -> bool {
    let generated = item.generated.as_deref();
    generated.and_then(GenerationInfo::script).is_some()
}

/// Maps each pipeline step onto its share of the job's progress bar.
struct Progress {
    job: JobProgress,
    steps: Vec<(Step, f32)>,
}

#[derive(Clone, Copy, PartialEq)]
enum Step {
    Audio,
    Speech,
    Transcript,
    Words,
    Speakers,
    Sounds,
    Beats,
    Shots,
}

impl Progress {
    /// Whether the job was cancelled: it stops, keeping nothing.
    fn cancelled(&self) -> bool {
        self.job.cancelled()
    }

    /// Callback for `step` reporting its own 0..1; it does nothing for a
    /// step the job doesn't run.
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
        let job = at.map(|_| self.job.clone());
        move |f: f32| {
            if let Some(job) = &job {
                job.set(from + span * f.clamp(0.0, 1.0))
            }
        }
    }
}

/// What a job runs, fixed when it starts.
struct Plan {
    steps: AnalysisSteps,
    /// Transcribes in place of the local speech models.
    online: Option<Arc<dyn SpeechService>>,
    /// The sound pass runs, with sound search and beats when these say.
    sounds: bool,
    search: bool,
    beats: bool,
    /// The parts made again; the others are kept from the result the job
    /// adds to. Speech is voice activity, the transcript and speakers;
    /// hearing is sounds, music and sound vectors. Shot changes run when
    /// `steps.shots` says.
    speech: bool,
    hearing: bool,
}

impl Plan {
    /// The plan for analyzing `item` with the steps configured now.
    fn for_item(shared: &Shared, item: &MediaItem, sound_only: bool) -> Self {
        let script = has_script(item) && !sound_only;
        Self::new(&lock(&shared.config), sound_only, script)
    }

    fn new(config: &AnalysisConfig, sound_only: bool, script: bool) -> Self {
        let mut steps = config.steps;
        if sound_only {
            steps.transcript = false;
            steps.speakers = false;
            steps.shots = false;
        }
        // A step runs only with its model on disk.
        let have = |task: ModelTask| task.model().is_downloaded();
        let online = config.speech.is_some() && !script;
        if !online && !script {
            steps.transcript &= have(ModelTask::Transcription);
            steps.speakers &= have(ModelTask::Speakers);
        }
        let sounds = cfg!(feature = "sounds") && steps.sounds && have(ModelTask::SoundTagging);
        Self {
            online: config
                .speech
                .clone()
                .filter(|_| steps.transcript && !script),
            sounds,
            search: sounds && steps.sound_search() && have(ModelTask::SoundSearch),
            beats: sounds && steps.beats() && have(ModelTask::Beats),
            steps,
            speech: true,
            hearing: true,
        }
    }

    /// The steps it runs: what its result covers when none fails.
    fn ran(&self) -> AnalysisSteps {
        AnalysisSteps {
            transcript: self.steps.transcript,
            speakers: self.steps.speakers(),
            sounds: self.sounds,
            beats: self.beats,
            sound_search: self.search,
            shots: self.steps.shots,
        }
    }

    /// Only the parts a result covering `have` lacks, each run whole.
    fn missing(mut self, have: &AnalysisSteps) -> Self {
        let wanted = self.ran();
        let lacks = |want: bool, has: bool| want && !has;
        self.hearing = lacks(wanted.sounds, have.sounds)
            || lacks(wanted.beats, have.beats())
            || lacks(wanted.sound_search, have.sound_search());
        // Voice activity is heard again with the audio unless a transcript
        // goes with it.
        self.speech = lacks(wanted.transcript, have.transcript)
            || lacks(wanted.speakers, have.speakers())
            || (self.hearing && !have.transcript);
        if !self.speech {
            self.steps.transcript = false;
            self.steps.speakers = false;
            self.online = None;
        }
        if !self.hearing {
            (self.sounds, self.search, self.beats) = (false, false, false);
        }
        self.steps.shots &= !have.shots;
        self
    }

    /// What a result covering `have` that this plan ran on covers, given
    /// the parts that failed.
    fn covered(&self, have: &AnalysisSteps, failed: &Failed) -> AnalysisSteps {
        let ran = self.ran();
        let mut steps = *have;
        if self.speech {
            steps.transcript = ran.transcript && !failed.speech;
            steps.speakers = ran.speakers && !failed.speech;
        }
        if self.hearing {
            steps.sounds = ran.sounds && !failed.hearing;
            steps.beats = ran.beats && !failed.hearing;
            steps.sound_search = ran.sound_search && !failed.hearing;
        }
        if ran.shots {
            steps.shots = !failed.shots;
        }
        steps
    }
}

/// The parts of a job that failed.
#[derive(Default)]
struct Failed {
    speech: bool,
    hearing: bool,
    shots: bool,
}

/// The whole pipeline for one item, or the parts of it the cached result
/// lacks unless the task replaces it. A step that fails leaves its part out
/// (or a stand-in: the energy VAD's speech, one speaker for all) and the
/// rest goes on; such a result is kept in memory for this session, without
/// the failed part among its steps, but only complete ones are written to
/// disk. Cancelled, it stops at the next step and keeps nothing: what a
/// step that was cut short made is not a result.
fn run(shared: &Shared, task: &Task, job: &JobProgress) -> Result<(), String> {
    let item = &task.item;
    let fp = &task.fingerprint;
    let generated = item.generated.as_deref();
    let script = generated.and_then(GenerationInfo::script);
    let script = script.filter(|_| !task.options.sound_only);
    let plan = Plan::new(
        &lock(&shared.config),
        task.options.sound_only,
        script.is_some(),
    );
    // What is made already: by a job queued before this one, too.
    let old = shared.entry(fp).filter(|_| !task.options.refresh);
    let have = old.as_ref().map_or(AnalysisSteps::NONE, |c| c.steps);
    let plan = match &old {
        Some(_) if have.covers(&plan.ran()) => return Ok(()),
        Some(_) => plan.missing(&have),
        None => plan,
    };
    let steps = plan.steps;
    let script = script.filter(|_| steps.transcript);
    let duration = item.info.as_ref().map(|i| i.duration);
    let audio_wanted = has_audio(item) && (plan.speech || plan.hearing);
    let video_wanted = has_video(item) && steps.shots;
    let stream = choose_stream(shared, item, &task.options);
    let mut weights = Vec::new();
    if audio_wanted {
        weights.push((Step::Audio, 1.0));
        if plan.online.is_some() {
            weights.push((Step::Transcript, 6.0));
        } else if plan.speech {
            if cfg!(feature = "whisper") {
                weights.push((Step::Speech, 0.5));
            }
            if steps.transcript {
                if script.is_none() {
                    weights.push((Step::Transcript, 6.0));
                }
                if cfg!(feature = "aligner") {
                    weights.push((Step::Words, 3.0));
                }
                if steps.speakers() && script.is_none() {
                    weights.push((Step::Speakers, 3.0));
                }
            }
        }
        if plan.sounds {
            weights.push((Step::Sounds, 4.0));
            if plan.beats {
                weights.push((Step::Beats, 1.0));
            }
        }
    }
    if video_wanted {
        weights.push((Step::Shots, 2.0));
    }
    let progress = Progress {
        job: job.clone(),
        steps: weights,
    };

    // The parts made again start empty; the rest is the old result's.
    let mut analysis = old.map_or_else(Analysis::default, |c| (*c.analysis).clone());
    if plan.speech {
        analysis.speech.clear();
        analysis.transcript = None;
    }
    if plan.hearing {
        analysis.sounds.clear();
        analysis.music.clear();
    }
    if steps.shots {
        analysis.shot_changes.clear();
    }
    let mut problems: Vec<String> = Vec::new();
    let mut failed = Failed::default();
    // Made by the sound pass, when it runs.
    let mut vectors = None;
    // This build cannot transcribe: keep the rest for this session, but
    // leave the disk cache for a build that can.
    let mut unpersisted = false;
    let cancel = || job.cancelled();

    if audio_wanted {
        let audio = match ffmpeg::extract_audio(
            &item.path,
            stream,
            duration,
            &progress.step(Step::Audio),
            &cancel,
        ) {
            Ok(audio) => Some(audio),
            Err(e) if item.info.is_none() && ffmpeg::is_missing_stream(&e) => None,
            Err(e) => {
                problems.push(format!("audio: {e:#}"));
                failed.speech = true;
                failed.hearing = true;
                None
            }
        };
        if cancel() {
            return Err(CANCELLED.into());
        }
        if let Some(audio) = audio {
            let before = problems.len();
            let (levels, sound) = align::measure(&audio);
            // What the provider heard besides words, for when the sound
            // models don't run.
            let mut events = Vec::new();
            if let Some(online) = &plan.online {
                match transcribe_online(
                    shared,
                    &**online,
                    task,
                    stream,
                    steps.speakers(),
                    &progress,
                ) {
                    // Its times stay as it aligned them: the edges measured
                    // on the audio are for Whisper's, which start late and
                    // end early, and pulled good ones off the words.
                    Ok((t, heard)) => {
                        analysis.speech = vad::from_words(&t.words);
                        events = heard;
                        analysis.transcript = Some(t);
                    }
                    Err(e) => {
                        problems.push(format!("transcript: {e:#}"));
                        analysis.speech = vad::speech_ranges(&audio);
                    }
                }
            } else if plan.speech {
                if cancel() {
                    return Err(CANCELLED.into());
                }
                let Voice { speech, stretches } =
                    listen(shared, &audio, &progress).unwrap_or_else(|e| {
                        problems.push(format!("voice activity: {e:#}"));
                        Voice::energy(&audio)
                    });
                analysis.speech = speech;
                if cancel() {
                    return Err(CANCELLED.into());
                }
                if let Some(script) = script {
                    let end = Time::from_secs_f64(audio.len() as f64 / ffmpeg::SAMPLE_RATE as f64);
                    let mut t = written(task, stream, script);
                    align::spread(&mut t.words, &analysis.speech, Time::ZERO, end);
                    let words = progress.step(Step::Words);
                    match follow_script(
                        shared,
                        &audio,
                        &analysis.speech,
                        &sound,
                        &mut t.words,
                        &words,
                    ) {
                        Ok(true) => {}
                        Ok(false) => align::refine(&mut t.words, &levels, &sound),
                        Err(e) => {
                            align::refine(&mut t.words, &levels, &sound);
                            problems.push(format!("word alignment: {e:#}"));
                        }
                    }
                    one_speaker(&mut t);
                    analysis.transcript = Some(t);
                } else if steps.transcript {
                    let transcript = transcript(
                        shared,
                        task,
                        stream,
                        &audio,
                        &analysis.speech,
                        stretches.as_deref(),
                        &progress,
                    );
                    if cancel() {
                        return Err(CANCELLED.into());
                    }
                    match transcript {
                        Ok(Some(mut t)) => {
                            // Edges measured on the audio only where the
                            // aligner can't place the words: measured
                            // first, they moved the last word of a phrase
                            // over the pause after it, and the aligner
                            // followed.
                            let words = progress.step(Step::Words);
                            match align_words(shared, &audio, &t.language, &mut t.words, &words) {
                                Ok(true) => {}
                                Ok(false) => align::refine(&mut t.words, &levels, &sound),
                                Err(e) => {
                                    align::refine(&mut t.words, &levels, &sound);
                                    problems.push(format!("word alignment: {e:#}"));
                                }
                            }
                            if cancel() {
                                return Err(CANCELLED.into());
                            }
                            // After the aligner, so that a hole is where no
                            // word is.
                            let speech = &analysis.speech;
                            if let Err(e) = fill_holes(shared, &audio, speech, &mut t, &cancel) {
                                problems.push(format!("transcript: {e:#}"));
                            }
                            if cancel() {
                                return Err(CANCELLED.into());
                            }
                            // After the aligner, so each word is heard
                            // where it is.
                            if !steps.speakers() {
                                one_speaker(&mut t);
                            } else if let Err(e) =
                                attribute_speakers(shared, &audio, &mut t, &progress)
                            {
                                problems.push(format!("speakers: {e:#}"));
                            }
                            analysis.transcript = Some(t);
                        }
                        Ok(None) => unpersisted = true,
                        Err(e) => problems.push(format!("transcript: {e:#}")),
                    }
                }
            }
            if cancel() {
                return Err(CANCELLED.into());
            }
            failed.speech = problems.len() > before;
            drop(audio);
            analysis.levels = Some(levels);
            if plan.sounds {
                let heard = hear(shared, task, stream, &plan, &mut analysis, &progress);
                match heard {
                    Ok(made) => vectors = made,
                    Err((made, e)) => {
                        vectors = made;
                        failed.hearing = true;
                        problems.push(format!("sounds: {e:#}"));
                    }
                }
            } else if plan.hearing && steps.sounds {
                (analysis.sounds, analysis.music) = from_events(events);
            }
        }
    }

    if video_wanted {
        if cancel() {
            return Err(CANCELLED.into());
        }
        match ffmpeg::shot_changes(
            &item.path,
            shared.options.scene_threshold,
            duration,
            &progress.step(Step::Shots),
            &cancel,
        ) {
            Ok(shots) => analysis.shot_changes = shots,
            Err(e) if item.info.is_none() && ffmpeg::is_missing_stream(&e) => {}
            Err(e) => {
                failed.shots = true;
                problems.push(format!("shot changes: {e:#}"));
            }
        }
    }

    // A step cut short failed for it: nothing here is kept, so asking
    // again runs it all as if this job had not been.
    if cancel() {
        return Err(CANCELLED.into());
    }
    analysis.version = ANALYSIS_VERSION;
    let mut covered = plan.covered(&have, &failed);
    if unpersisted {
        // Nothing was transcribed: a build that can adds the transcript.
        (covered.transcript, covered.speakers) = (false, false);
    }
    let complete = problems.is_empty();
    if complete && !unpersisted {
        // The vectors first: when they can't be written, the result on
        // disk stays the one beside the vectors there.
        let vectors_stored = match (&vectors, plan.hearing) {
            (_, false) => Ok(()),
            (Some(v), true) => cache::store_vectors(fp, v),
            (None, true) => cache::remove_vectors(fp),
        };
        let stored = vectors_stored
            .map_err(|e| format!("sound vectors: {e:#}"))
            .and_then(|()| {
                cache::store(fp, &analysis, &covered).map_err(|e| format!("cache: {e:#}"))
            });
        if let Err(e) = stored {
            problems.push(e);
        }
    }
    // The vectors of the sound pass that ran, or none: older ones are of
    // another run, and maybe another model.
    if plan.hearing {
        lock(&shared.vectors).insert(fp.clone(), vectors.map(Arc::new));
    }
    let cached = Cached {
        analysis: Arc::new(analysis),
        steps: covered,
    };
    lock(&shared.results).insert(fp.clone(), cached);
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("; "))
    }
}

/// Where someone talks in an item's audio.
struct Voice {
    speech: Vec<TimeRange>,
    /// The stretches of it Whisper hears, in samples; `None` when Silero
    /// did not hear them.
    stretches: Option<Vec<Range<usize>>>,
}

impl Voice {
    /// The energy VAD's speech, which takes music for speech.
    fn energy(audio: &[f32]) -> Self {
        Self {
            speech: vad::speech_ranges(audio),
            stretches: None,
        }
    }
}

/// What the voice activity model hears in `audio`; the energy VAD stands
/// in for it in a build without it, or when the model is missing.
#[cfg(feature = "whisper")]
fn listen(shared: &Shared, audio: &[f32], progress: &Progress) -> anyhow::Result<Voice> {
    if !ModelTask::VoiceActivity.model().is_downloaded() {
        return Ok(Voice::energy(audio));
    }
    let heard = shared.voice.hear(audio, &progress.step(Step::Speech))?;
    Ok(Voice {
        speech: heard.ranges,
        stretches: Some(heard.stretches),
    })
}

#[cfg(not(feature = "whisper"))]
fn listen(_shared: &Shared, audio: &[f32], _progress: &Progress) -> anyhow::Result<Voice> {
    Ok(Voice::energy(audio))
}

/// The words in the `stretches` of `audio` Silero heard; without them,
/// only an item the energy VAD finds no `speech` in is transcribed (as
/// nothing). `Ok(None)` when this build cannot transcribe.
#[cfg(feature = "whisper")]
fn transcript(
    shared: &Shared,
    task: &Task,
    stream: u32,
    audio: &[f32],
    speech: &[TimeRange],
    stretches: Option<&[Range<usize>]>,
    progress: &Progress,
) -> anyhow::Result<Option<Transcript>> {
    let language = task
        .options
        .language
        .as_deref()
        .and_then(|tag| iso_639_1(shared, tag));
    let mut words = Vec::new();
    let mut detected = language.map(str::to_string);
    // Whisper invents words in silence and over music; it hears only the
    // speech Silero finds, and only runs when someone talks.
    let stretches = match stretches {
        Some(stretches) => stretches,
        None if speech.is_empty() => &[],
        None => anyhow::bail!("Whisper waits for the voice activity model"),
    };
    if !stretches.is_empty() {
        let out = shared.transcriber.get().transcribe(
            audio,
            stretches,
            language,
            Arc::new(progress.step(Step::Transcript)),
        )?;
        words = out.words;
        detected = Some(out.language);
    }
    Ok(Some(Transcript {
        fingerprint: task.fingerprint.clone(),
        audio_stream: stream,
        language: detected.unwrap_or_else(|| "und".into()),
        words,
        speakers: Vec::new(),
        engine: Some(ModelTask::Transcription.model().id.to_owned()),
    }))
}

/// The transcript of a voiceover, as `script` is written: no times yet, and
/// no language, which nothing here hears.
fn written(task: &Task, stream: u32, script: &str) -> Transcript {
    let words = cloud::spoken_words(script).into_iter().map(|text| Word {
        range: TimeRange::new(Time::ZERO, Time::ZERO),
        text,
        confidence: 1.0,
        speaker: None,
    });
    Transcript {
        fingerprint: task.fingerprint.clone(),
        audio_stream: stream,
        language: "und".into(),
        words: words.collect(),
        speakers: Vec::new(),
        engine: Some("script".into()),
    }
}

/// The words a provider heard in `stream` of the item, with its voices
/// when `voices` (else one speaker for all), and the sounds it noted
/// between them. The audio goes to it as lossless mono FLAC.
fn transcribe_online(
    shared: &Shared,
    online: &dyn SpeechService,
    task: &Task,
    stream: u32,
    voices: bool,
    progress: &Progress,
) -> anyhow::Result<(Transcript, Vec<Sound>)> {
    let item = &task.item;
    let step = progress.step(Step::Transcript);
    let upload = cache::upload(&task.fingerprint)?;
    let rate = item
        .info
        .as_ref()
        .and_then(|i| i.audio.get(stream as usize))
        .map(|a| a.sample_rate)
        .filter(|r| *r > 0);
    let duration = item.info.as_ref().map(|i| i.duration);
    let language = task
        .options
        .language
        .as_deref()
        .and_then(|tag| iso_639_1(shared, tag));
    let heard = ffmpeg::encode_flac(
        &item.path,
        stream,
        rate,
        UPLOAD_MAX_RATE,
        &upload,
        duration,
        &|f| step(f * 0.1),
        &|| progress.cancelled(),
    )
    .and_then(|()| {
        if progress.cancelled() {
            anyhow::bail!(CANCELLED);
        }
        let request = SpeechRequest {
            audio: &upload,
            language,
            speakers: voices,
        };
        online
            .transcribe(&request, &|f| step(0.1 + 0.9 * f))
            .map_err(|e| anyhow::anyhow!("{}: {e}", online.name()))
    });
    let _ = std::fs::remove_file(&upload);
    let heard = heard?;
    let mut heard_words: Vec<_> = heard.words.into_iter().collect();
    heard_words.sort_by_key(|w| w.range.start);
    let (mut words, labels): (Vec<Word>, Vec<Option<usize>>) = heard_words
        .into_iter()
        .map(|w| {
            let word = Word {
                range: w.range,
                text: w.text,
                confidence: w.confidence,
                speaker: None,
            };
            (word, w.speaker)
        })
        .unzip();
    let mut transcript = Transcript {
        fingerprint: task.fingerprint.clone(),
        audio_stream: stream,
        language: iso_639_1(shared, &heard.language)
            .map(str::to_owned)
            .unwrap_or(heard.language),
        words: Vec::new(),
        speakers: Vec::new(),
        engine: Some(heard.engine),
    };
    if voices && labels.iter().any(Option::is_some) {
        let count = labels.iter().flatten().max().map_or(0, |m| m + 1);
        let labels = labels.into_iter().map(|l| l.map(|i| (i, 1.0))).collect();
        transcript.speakers = speakers::apply((count, labels), &mut words);
        transcript.words = words;
    } else {
        transcript.words = words;
        one_speaker(&mut transcript);
    }
    Ok((transcript, heard.events))
}

/// A provider's audio events as the sounds and music of the sound script,
/// for when the sound models don't run: music without tags or beats.
fn from_events(events: Vec<Sound>) -> (Vec<Sound>, Vec<Music>) {
    let mut sounds = Vec::new();
    let mut music: Vec<Music> = Vec::new();
    for event in events {
        if !event.label.contains("music") {
            sounds.push(event);
            continue;
        }
        match music.last_mut() {
            Some(m) if event.range.start <= m.range.end => {
                m.range.end = m.range.end.max(event.range.end)
            }
            _ => music.push(Music {
                range: event.range,
                tags: Some(event.label)
                    .filter(|l| l != "music")
                    .into_iter()
                    .collect(),
                beats: Vec::new(),
                downbeats: Vec::new(),
            }),
        }
    }
    (sounds, music)
}

/// The sound vectors [`hear`] made; when it failed, with why.
type Heard = Result<Option<SoundVectors>, (Option<SoundVectors>, anyhow::Error)>;

/// Sounds, music and beats for `analysis`, whose levels are measured, and
/// its sound vectors; sound vectors and beats only when `plan` says. A
/// model that could not be had is an error; what the others heard is kept.
#[cfg(feature = "sounds")]
fn hear(
    shared: &Shared,
    task: &Task,
    stream: u32,
    plan: &Plan,
    analysis: &mut Analysis,
    progress: &Progress,
) -> Heard {
    let pass = sounds::Pass {
        search: plan.search,
        beats: plan.beats,
    };
    let heard = shared
        .sounds
        .hear(
            &task.item.path,
            stream,
            task.item.info.as_ref().map(|i| i.duration),
            analysis.levels.as_ref(),
            pass,
            &progress.step(Step::Sounds),
            &progress.step(Step::Beats),
            &|| progress.cancelled(),
        )
        .map_err(|e| (None, e))?;
    analysis.sounds = heard.sounds;
    analysis.music = heard.music;
    if !heard.problems.is_empty() {
        let e = anyhow::anyhow!("{}", heard.problems.join("; "));
        return Err((heard.vectors, e));
    }
    Ok(heard.vectors)
}

#[cfg(not(feature = "sounds"))]
fn hear(
    _shared: &Shared,
    _task: &Task,
    _stream: u32,
    _plan: &Plan,
    _analysis: &mut Analysis,
    _progress: &Progress,
) -> Heard {
    Ok(None)
}

/// Fit `words`, said in `language`, to the audio with the alignment model;
/// whether it did. A language it does not know keeps the edges measured
/// on the audio, for good.
#[cfg(feature = "aligner")]
fn align_words(
    shared: &Shared,
    audio: &[f32],
    language: &str,
    words: &mut [Word],
    progress: &dyn Fn(f32),
) -> anyhow::Result<bool> {
    if words.is_empty()
        || !ModelTask::WordTiming.model().is_downloaded()
        || !shared.aligner.supports(language)
    {
        return Ok(false);
    }
    shared.aligner.align(audio, words, progress)?;
    Ok(true)
}

#[cfg(not(feature = "aligner"))]
fn align_words(
    _shared: &Shared,
    _audio: &[f32],
    _language: &str,
    _words: &mut [Word],
    _progress: &dyn Fn(f32),
) -> anyhow::Result<bool> {
    Ok(false)
}

/// A stretch the aligner hears of a voiceover, with the words expected in
/// its first [`EXPECTED`]. Words it puts after [`KEPT`] go again with the
/// next stretch, so a word the model ran out of audio for is heard again.
#[cfg(feature = "aligner")]
const HEARD: Time = Time(FLICKS_PER_SECOND * 30);
#[cfg(feature = "aligner")]
const EXPECTED: Time = Time(FLICKS_PER_SECOND * 25);
#[cfg(feature = "aligner")]
const KEPT: Time = Time(FLICKS_PER_SECOND * 27);
/// A stretch with more of its words squeezed into one step than this, apart
/// from its longest run of them, doesn't follow the audio. Well heard it is
/// a tenth at most; a passage of the script that isn't said, squeezed where
/// it is, makes one run.
#[cfg(feature = "aligner")]
const LOST: f32 = 0.15;

/// Fit a script's `words` to the audio with the alignment model, a stretch
/// at a time, each from where the last word placed ended; whether it did.
/// How long a script takes is only known roughly, and that adds up over
/// minutes: an error at one stretch doesn't reach the next.
///
/// A stretch can't hear past a long pause, so pauses are shortened first
/// ([`Shortened`]). A passage of the script that isn't said leaves the
/// words after it late, and a stretch never hears before where the last
/// one ended; such a stretch squeezes its words, and the words whose audio
/// is behind it are left where it starts ([`catch_up`]).
#[cfg(feature = "aligner")]
fn follow_script(
    shared: &Shared,
    audio: &[f32],
    speech: &[TimeRange],
    sound: &[bool],
    words: &mut [Word],
    progress: &dyn Fn(f32),
) -> anyhow::Result<bool> {
    if !ModelTask::WordTiming.model().is_downloaded() {
        return Ok(false);
    }
    let read = Shortened::new(audio, speech, sound);
    let speech: Vec<TimeRange> = speech.iter().map(|r| read.shorten(*r)).collect();
    let hear = |audio: &[f32], run: &mut [Word]| shared.aligner.align(audio, run, &|_| {});
    let end = read.end();
    let (mut from, mut done) = (Time::ZERO, 0);
    // Where the stretch before started.
    let mut before = (Time::ZERO, 0);
    while done < words.len() {
        progress(done as f32 / words.len() as f32);
        let mut heard = Stretch::hear(&read.audio, &speech, &mut words[done..], from, &hear)?;
        if heard.lost() {
            let skip = catch_up(&read.audio, &speech, &mut words[done..], from, &hear)?;
            if skip > 0 {
                for word in &mut words[done..done + skip] {
                    word.range = TimeRange::new(from, from);
                }
                // The passage not said is likely in the stretch before:
                // heard again up to here, the model has to squeeze it.
                let (lo, back) = before;
                let again = &mut words[back..done + skip];
                hear_between(&read.audio, &speech, again, lo, from, &hear)?;
                done += skip;
                heard = Stretch::hear(&read.audio, &speech, &mut words[done..], from, &hear)?;
            }
        }
        for (word, placed) in words[done..].iter_mut().zip(&heard.run[..heard.kept]) {
            word.range = placed.range.shift(heard.start);
        }
        before = (from, done);
        done += heard.kept;
        from = words[done - 1].range.end.min(end);
    }
    for word in words.iter_mut() {
        word.range = read.restore(read.one_side(word.range, &speech));
    }
    progress(1.0);
    Ok(true)
}

#[cfg(not(feature = "aligner"))]
fn follow_script(
    _shared: &Shared,
    _audio: &[f32],
    _speech: &[TimeRange],
    _sound: &[bool],
    _words: &mut [Word],
    _progress: &dyn Fn(f32),
) -> anyhow::Result<bool> {
    Ok(false)
}

/// The aligner hearing `audio` with a run of words timed in it.
#[cfg(feature = "aligner")]
type Hear<'a> = dyn Fn(&[f32], &mut [Word]) -> anyhow::Result<()> + 'a;

/// The sample of 16 kHz `audio` at `t`.
#[cfg(feature = "aligner")]
fn at(audio: &[f32], t: Time) -> usize {
    ((t.as_secs_f64() * ffmpeg::SAMPLE_RATE as f64) as usize).min(audio.len())
}

/// How long 16 kHz `audio` lasts.
#[cfg(feature = "aligner")]
fn length(audio: &[f32]) -> Time {
    Time::from_secs_f64(audio.len() as f64 / ffmpeg::SAMPLE_RATE as f64)
}

/// One stretch of [`follow_script`]: the words heard, times from `start`.
#[cfg(feature = "aligner")]
struct Stretch {
    start: Time,
    run: Vec<Word>,
    /// How many of `run` to keep; the rest go again with the next one.
    kept: usize,
}

#[cfg(feature = "aligner")]
impl Stretch {
    /// [`HEARD`] from where the speech goes on after `from`, with the
    /// `words` expected in its first [`EXPECTED`]; `words` get rough times.
    fn hear(
        audio: &[f32],
        speech: &[TimeRange],
        words: &mut [Word],
        from: Time,
        hear: &Hear,
    ) -> anyhow::Result<Self> {
        Self::hear_for(audio, speech, words, from, hear, (HEARD, EXPECTED, KEPT))
    }

    fn hear_for(
        audio: &[f32],
        speech: &[TimeRange],
        words: &mut [Word],
        from: Time,
        hear: &Hear,
        (heard, expected, kept): (Time, Time, Time),
    ) -> anyhow::Result<Self> {
        let end = length(audio);
        align::spread(words, speech, from, end);
        let start = words[0].range.start;
        let stop = (start + heard).min(end);
        let last = stop == end;
        let within = |w: &&Word| last || w.range.end <= start + expected;
        let taken = words.iter().take_while(within).count().max(1);
        let mut run = words[..taken].to_vec();
        for word in &mut run {
            word.range = word.range.shift(-start);
        }
        hear(&audio[at(audio, start)..at(audio, stop)], &mut run)?;
        let kept = if last {
            taken
        } else {
            let placed = run.iter().take_while(|w| w.range.end <= kept).count();
            placed.max(taken.div_ceil(2))
        };
        Ok(Self { start, run, kept })
    }

    fn squeezed(&self) -> (f32, f32) {
        squeezed(&self.run[..self.kept])
    }

    /// Whether this stretch doesn't follow the audio.
    fn lost(&self) -> bool {
        self.kept >= 8 && self.squeezed().1 > LOST
    }
}

/// The share of `words` the model squeezed into one step, as it does words
/// it has no audio for, and of those apart from their longest run.
#[cfg(feature = "aligner")]
fn squeezed(words: &[Word]) -> (f32, f32) {
    let (mut all, mut run, mut longest) = (0, 0, 0);
    for word in words {
        if word.range.duration() <= Time::from_millis(30) {
            all += 1;
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    let share = |n: usize| n as f32 / words.len().max(1) as f32;
    (share(all), share(all - longest))
}

/// Hear `words` again between `from` and `to`, where the words around them
/// are; kept when the model follows the audio there but for one run of
/// words squeezed, the passage not said.
#[cfg(feature = "aligner")]
fn hear_between(
    audio: &[f32],
    speech: &[TimeRange],
    words: &mut [Word],
    from: Time,
    to: Time,
    hear: &Hear,
) -> anyhow::Result<()> {
    if words.is_empty() || to <= from {
        return Ok(());
    }
    let mut run = words.to_vec();
    align::spread(&mut run, speech, from, to);
    let start = run[0].range.start.min(to);
    for word in &mut run {
        word.range = word.range.shift(-start);
    }
    hear(&audio[at(audio, start)..at(audio, to)], &mut run)?;
    if squeezed(&run).1 <= LOST / 2.0 {
        for (word, placed) in words.iter_mut().zip(&run) {
            word.range = placed.range.shift(start);
        }
    }
    Ok(())
}

/// How many of `words` were said before `from`, when a stretch from there
/// is [lost](Stretch::lost): a passage of the script before them wasn't
/// said, so their audio went to it. Short stretches are heard from words
/// further on, and the first that follows the audio says: one too far
/// starts with speech that isn't its words, which the model doesn't
/// squeeze, and leaves words out, so the fewest. None (0) when a short one
/// from the first word follows the audio too.
#[cfg(feature = "aligner")]
fn catch_up(
    audio: &[f32],
    speech: &[TimeRange],
    words: &mut [Word],
    from: Time,
    hear: &Hear,
) -> anyhow::Result<usize> {
    /// Words tried further on, by this step, up to this many.
    const STEP: usize = 3;
    const FURTHEST: usize = 60;
    /// Followed: fewer words squeezed than this.
    const FOLLOWED: f32 = 0.08;
    let short = (
        Time(FLICKS_PER_SECOND * 12),
        Time(FLICKS_PER_SECOND * 10),
        Time(FLICKS_PER_SECOND * 11),
    );
    let furthest = FURTHEST.min(words.len().saturating_sub(8));
    let mut follows = |skip: usize| -> anyhow::Result<Option<bool>> {
        let words = &mut words[skip..];
        let heard = Stretch::hear_for(audio, speech, words, from, hear, short)?;
        Ok((heard.kept >= 5).then(|| heard.squeezed().0 <= FOLLOWED))
    };
    if follows(0)? != Some(false) {
        return Ok(0);
    }
    for skip in (STEP..=furthest).step_by(STEP) {
        if follows(skip)? == Some(true) {
            return Ok(skip);
        }
    }
    Ok(0)
}

/// A voiceover's audio with each long silence, where neither the voice
/// activity nor the levels hear anything, shortened to [`Shortened::LEFT`]
/// either side of its middle: a stretch hears 30 s of the read, not of the
/// pause, and so hears the words after it too.
#[cfg(feature = "aligner")]
struct Shortened<'a> {
    audio: std::borrow::Cow<'a, [f32]>,
    /// The silences taken out, and where each was in the shortened audio.
    cuts: Vec<(TimeRange, Time)>,
}

#[cfg(feature = "aligner")]
impl<'a> Shortened<'a> {
    /// Silences longer than this are shortened…
    const LONG: Time = Time(FLICKS_PER_SECOND * 10);
    /// …to this much at each end.
    const LEFT: Time = Time(FLICKS_PER_SECOND * 3);

    /// `sound` comes from [`align::measure`] on `audio`.
    fn new(audio: &'a [f32], speech: &[TimeRange], sound: &[bool]) -> Self {
        let frame = |t: Time| (t.0 / Levels::FRAME.0).clamp(0, sound.len() as i64) as usize;
        let mut quiet: Vec<bool> = sound.iter().map(|s| !s).collect();
        for r in speech {
            let to = frame(r.end + Levels::FRAME).max(frame(r.start));
            quiet[frame(r.start)..to].fill(false);
        }
        let (long, left) = (frame(Self::LONG), frame(Self::LEFT));
        // Only between sounds: a silence at either end holds up nothing.
        let first = quiet.iter().position(|q| !q).unwrap_or(quiet.len());
        let last = quiet.iter().rposition(|q| !q).map_or(0, |i| i + 1);
        let mut silences = Vec::new();
        let mut i = first;
        while i < last {
            let run = quiet[i..last].iter().take_while(|q| **q).count();
            if run > long {
                silences.push((i + left)..(i + run - left));
            }
            i += run.max(1);
        }
        if silences.is_empty() {
            return Self {
                audio: audio.into(),
                cuts: Vec::new(),
            };
        }
        let samples = |f: usize| (f * (ffmpeg::SAMPLE_RATE as usize / 100)).min(audio.len());
        let time = |s: usize| Time::from_secs_f64(s as f64 / ffmpeg::SAMPLE_RATE as f64);
        let mut kept = Vec::with_capacity(audio.len());
        let mut cuts = Vec::new();
        let mut at = 0;
        for cut in silences {
            let (from, to) = (samples(cut.start), samples(cut.end));
            kept.extend_from_slice(&audio[at..from]);
            cuts.push((TimeRange::new(time(from), time(to)), time(kept.len())));
            at = to;
        }
        kept.extend_from_slice(&audio[at..]);
        Self {
            audio: kept.into(),
            cuts,
        }
    }

    fn end(&self) -> Time {
        length(&self.audio)
    }

    /// `range` of the audio, in the shortened audio; it holds no silence
    /// taken out.
    fn shorten(&self, range: TimeRange) -> TimeRange {
        let before = |t: Time| {
            let cut = self.cuts.iter().filter(|(c, _)| c.end <= t);
            Time(cut.map(|(c, _)| c.duration().0).sum())
        };
        TimeRange::new(
            range.start - before(range.start),
            range.end - before(range.end),
        )
    }

    /// A word placed over where a silence was taken out, which no word is,
    /// cut back to the side of it with more `speech`.
    fn one_side(&self, range: TimeRange, speech: &[TimeRange]) -> TimeRange {
        let Some(&(_, at)) = self
            .cuts
            .iter()
            .find(|(_, at)| range.start < *at && *at < range.end)
        else {
            return range;
        };
        let heard = |side: TimeRange| -> i64 {
            let heard = speech.iter().filter_map(|r| r.intersect(&side));
            heard.map(|r| r.duration().0).sum()
        };
        let (before, after) = (
            TimeRange::new(range.start, at),
            TimeRange::new(at, range.end),
        );
        if heard(after) >= heard(before) {
            let resumes = speech.iter().map(|r| r.start).find(|s| *s >= at);
            TimeRange::new(resumes.unwrap_or(at).min(range.end), range.end)
        } else {
            let stops = speech.iter().rev().map(|r| r.end).find(|e| *e <= at);
            TimeRange::new(range.start, stops.unwrap_or(at).max(range.start))
        }
    }

    /// `range` of the shortened audio, in the audio.
    fn restore(&self, range: TimeRange) -> TimeRange {
        let after = |t: Time| {
            let cut = self.cuts.iter().filter(|(_, at)| *at < t);
            t + Time(cut.map(|(c, _)| c.duration().0).sum())
        };
        TimeRange::new(after(range.start), after(range.end))
    }
}

/// Hear again, on its own, the `speech` no word of `transcript` was heard
/// in, and add the words found there, aligned. Whisper now and then gives
/// one sentence a segment that runs on over seconds of speech it skips, or
/// takes the short last phrase of a stretch for silence. Stops between
/// holes once `cancel` says so.
#[cfg(feature = "whisper")]
fn fill_holes(
    shared: &Shared,
    audio: &[f32],
    speech: &[TimeRange],
    transcript: &mut Transcript,
    cancel: &dyn Fn() -> bool,
) -> anyhow::Result<()> {
    let holes = vad::holes(speech, &transcript.words);
    if holes.is_empty() {
        return Ok(());
    }
    let sample =
        |t: Time| ((t.as_secs_f64() * ffmpeg::SAMPLE_RATE as f64) as usize).min(audio.len());
    let language = Some(transcript.language.as_str()).filter(|l| *l != "und");
    let transcriber = shared.transcriber.get();
    for hole in holes {
        if cancel() {
            anyhow::bail!(CANCELLED);
        }
        // The hole's audio alone: the aligner looks for words seconds
        // around where they are said, and around a hole is speech already
        // heard, where it would put the few words found.
        let heard = &audio[sample(hole.start)..sample(hole.end)];
        if heard.is_empty() {
            continue;
        }
        let all = 0..heard.len();
        let mut found = transcriber
            .transcribe(
                heard,
                std::slice::from_ref(&all),
                language,
                Arc::new(|_| {}),
            )?
            .words;
        align_words(shared, heard, &transcript.language, &mut found, &|_| {})?;
        let at = Time::from_secs_f64(sample(hole.start) as f64 / ffmpeg::SAMPLE_RATE as f64);
        for w in &mut found {
            w.range = w.range.shift(at);
        }
        transcript.words.extend(found);
    }
    transcript.words.sort_by_key(|w| w.range.start);
    Ok(())
}

#[cfg(not(feature = "whisper"))]
fn fill_holes(
    _shared: &Shared,
    _audio: &[f32],
    _speech: &[TimeRange],
    _transcript: &mut Transcript,
    _cancel: &dyn Fn() -> bool,
) -> anyhow::Result<()> {
    Ok(())
}

/// One speaker for every word of `transcript`.
fn one_speaker(transcript: &mut Transcript) {
    if transcript.words.is_empty() {
        transcript.speakers.clear();
        return;
    }
    let labels = (1, transcript.words.iter().map(|_| Some((0, 1.0))).collect());
    transcript.speakers = speakers::apply(labels, &mut transcript.words);
}

/// Tell the voices in `transcript` apart. When the diarizer can't, its
/// words get one speaker for all.
fn attribute_speakers(
    shared: &Shared,
    audio: &[f32],
    transcript: &mut Transcript,
    progress: &Progress,
) -> anyhow::Result<()> {
    if transcript.words.is_empty() {
        transcript.speakers.clear();
        return Ok(());
    }
    let labels = shared
        .diarizer
        .diarize(audio, &transcript.words, &progress.step(Step::Speakers));
    match labels {
        Ok(labels) => {
            transcript.speakers = speakers::apply(labels, &mut transcript.words);
            Ok(())
        }
        Err(e) => {
            let one = SingleSpeaker.diarize(audio, &transcript.words, &|_| {})?;
            transcript.speakers = speakers::apply(one, &mut transcript.words);
            Err(e)
        }
    }
}

#[cfg(not(feature = "whisper"))]
fn transcript(
    _shared: &Shared,
    _task: &Task,
    _stream: u32,
    _audio: &[f32],
    _speech: &[TimeRange],
    _stretches: Option<&[Range<usize>]>,
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
fn choose_stream(shared: &Shared, item: &MediaItem, options: &AnalyzeOptions) -> u32 {
    if let Some(n) = options.audio_stream {
        return n;
    }
    let code = |tag: &str| iso_639_1(shared, tag);
    let wanted = options.language.as_deref().and_then(code);
    let tagged = item.info.as_ref().zip(wanted).and_then(|(info, wanted)| {
        info.audio.iter().position(|a| {
            a.language
                .as_deref()
                .and_then(code)
                .is_some_and(|l| l == wanted)
        })
    });
    tagged.unwrap_or(0) as u32
}

/// The two-letter code of a 639-1 or common 639-2 tag, as the
/// transcription model knows it.
#[cfg_attr(not(feature = "whisper"), allow(unused_variables))]
fn iso_639_1(shared: &Shared, tag: &str) -> Option<&'static str> {
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
    if let Some(code) = shared.transcriber.get().language(tag) {
        return Some(code);
    }
    None
}

/// Where someone talks: the speech voice activity found, and every word
/// transcribed, so that a silence never takes the quiet last word of a
/// phrase that voice activity let go of.
fn talking(analysis: &Analysis) -> Vec<TimeRange> {
    let words = analysis
        .transcript
        .iter()
        .flat_map(|t| t.words.iter().map(|w| w.range));
    analysis.speech.iter().copied().chain(words).collect()
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
