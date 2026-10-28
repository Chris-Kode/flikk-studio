//! Local analysis: bundled Whisper transcription, voice activity, speakers,
//! sounds, music and beats, sound vectors, shot changes, silences and
//! loudness. Implements `koo_core::services::AnalysisService`.
//!
//! [`LocalAnalysis::analyze`] queues a job and returns at once; the jobs run
//! one at a time, in the order queued, on a [`JobQueue`]. Each runs the
//! pipeline (audio → the speech Silero hears, or the energy VAD's without
//! it, and levels → transcript of that speech → word edges on the audio →
//! word times from the aligner → speakers → sounds, music and sound vectors
//! → beats → shot changes) and caches the result by fingerprint, so an item
//! is analyzed once (principle 4) until the user analyzes it again. Results
//! of another [`ANALYSIS_VERSION`] are not read.

mod align;
mod cache;
mod ffmpeg;
#[cfg(feature = "sounds")]
mod sounds;
pub mod speakers;
mod vad;

use koo_core::analysis::{ANALYSIS_VERSION, Analysis, Loudness, SoundVectors, Transcript};
use koo_core::ids::JobId;
use koo_core::jobs::{JobProgress, JobQueue};
use koo_core::model::{Fingerprint, MediaItem, MediaKind};
use koo_core::services::{AnalysisService, JobStatus, ServiceError, ServiceResult};
use koo_core::time::{Time, TimeRange};
use koo_models::Task as ModelTask;
use speakers::{Diarizer, SingleSpeaker};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

pub use ffmpeg::binary as ffmpeg_binary;

/// Items analyzed at once. Each already keeps several cores busy, and the
/// large models a lot of memory.
const JOBS_AT_ONCE: usize = 1;

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
}

/// The [`AnalysisService`] that runs everything on this machine.
pub struct LocalAnalysis {
    shared: Arc<Shared>,
    /// Keyed by fingerprint, so the same file is analyzed once at a time.
    queue: JobQueue<Fingerprint>,
}

struct Task {
    item: MediaItem,
    fingerprint: Fingerprint,
    options: AnalyzeOptions,
}

struct Shared {
    options: Options,
    diarizer: Box<dyn Diarizer>,
    results: Mutex<HashMap<Fingerprint, Arc<Analysis>>>,
    /// Results kept in memory although a step failed; analyzed again on
    /// the next request.
    incomplete: Mutex<HashSet<Fingerprint>>,
    #[cfg(feature = "whisper")]
    voice: Box<dyn koo_models::voice_activity::VoiceActivity>,
    #[cfg(feature = "whisper")]
    transcriber: Picked<dyn koo_models::transcription::Transcriber>,
    #[cfg(feature = "aligner")]
    aligner: Box<dyn koo_models::word_timing::Aligner>,
    #[cfg(feature = "sounds")]
    sounds: sounds::SoundModels,
    /// Sound vectors read so far, by fingerprint.
    vectors: Mutex<HashMap<Fingerprint, Arc<SoundVectors>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The runner of the model a task runs on, made again when the user picks
/// another. A job keeps the runner it started with.
#[cfg(feature = "whisper")]
struct Picked<R: ?Sized> {
    task: ModelTask,
    make: fn(&koo_models::Model) -> Box<R>,
    runner: Mutex<(&'static str, Arc<R>)>,
}

#[cfg(feature = "whisper")]
impl<R: ?Sized> Picked<R> {
    fn new(task: ModelTask, make: fn(&koo_models::Model) -> Box<R>) -> Self {
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
        let diarizer = koo_models::speakers::runner(ModelTask::Speakers.model());
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
            incomplete: Mutex::default(),
            #[cfg(feature = "whisper")]
            voice: koo_models::voice_activity::runner(ModelTask::VoiceActivity.model()),
            #[cfg(feature = "whisper")]
            transcriber: Picked::new(ModelTask::Transcription, koo_models::transcription::runner),
            #[cfg(feature = "aligner")]
            aligner: koo_models::word_timing::runner(ModelTask::WordTiming.model()),
            #[cfg(feature = "sounds")]
            sounds: Default::default(),
            vectors: Mutex::default(),
        });
        Self {
            shared,
            queue: JobQueue::new("koo-analysis", JOBS_AT_ONCE),
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
        tasks
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
        if !options.refresh && self.shared.complete(&fp) {
            return self.queue.finished(label, Some(item.id), Ok(()));
        }
        let shared = self.shared.clone();
        let task = Task {
            item: item.clone(),
            fingerprint: fp.clone(),
            options,
        };
        self.queue.push(fp, label, Some(item.id), move |job| {
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

    fn analyze_again(&self, item: &MediaItem) -> JobId {
        let options = AnalyzeOptions {
            refresh: true,
            ..AnalyzeOptions::default()
        };
        self.analyze_with(item, options)
    }

    fn jobs(&self) -> Vec<JobStatus> {
        self.queue.jobs()
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
        let stream = choose_stream(&self.shared, item, &AnalyzeOptions::default());
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

    fn sound_vectors(&self, fingerprint: &Fingerprint) -> Option<Arc<SoundVectors>> {
        let shared = &self.shared;
        if let Some(v) = lock(&shared.vectors).get(fingerprint) {
            return Some(v.clone());
        }
        // Vectors beside a result of another version are not read either.
        shared.cached(fingerprint)?;
        let loaded = Arc::new(cache::load_vectors(fingerprint)?);
        lock(&shared.vectors).insert(fingerprint.clone(), loaded.clone());
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
    /// Whether full results are in memory or on disk.
    fn complete(&self, fp: &Fingerprint) -> bool {
        self.cached(fp).is_some() && !lock(&self.incomplete).contains(fp)
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
        let job = self.job.clone();
        move |f: f32| job.set(from + span * f.clamp(0.0, 1.0))
    }
}

/// The whole pipeline for one item. A step that fails leaves its part out
/// (or a stand-in: the energy VAD's speech, one speaker for all) and the
/// rest goes on; such a result is kept in memory for this session but only
/// complete ones are written to disk.
fn run(shared: &Shared, task: &Task, job: &JobProgress) -> Result<(), String> {
    let item = &task.item;
    let duration = item.info.as_ref().map(|i| i.duration);
    let audio_wanted = has_audio(item);
    let video_wanted = has_video(item);
    let stream = choose_stream(shared, item, &task.options);
    let mut steps = Vec::new();
    if audio_wanted {
        steps.push((Step::Audio, 1.0));
        if cfg!(feature = "whisper") {
            steps.push((Step::Speech, 0.5));
        }
        steps.push((Step::Transcript, 6.0));
        if cfg!(feature = "aligner") {
            steps.push((Step::Words, 3.0));
        }
        steps.push((Step::Speakers, 3.0));
        if cfg!(feature = "sounds") {
            steps.push((Step::Sounds, 4.0));
            steps.push((Step::Beats, 1.0));
        }
    }
    if video_wanted {
        steps.push((Step::Shots, 2.0));
    }
    let progress = Progress {
        job: job.clone(),
        steps,
    };

    let mut analysis = Analysis::default();
    let mut problems: Vec<String> = Vec::new();
    // This build cannot transcribe: keep the rest for this session, but
    // leave the disk cache for a build that can.
    let mut unpersisted = false;

    if audio_wanted {
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
                None
            }
        };
        if let Some(audio) = audio {
            let Voice { speech, stretches } =
                listen(shared, &audio, &progress).unwrap_or_else(|e| {
                    problems.push(format!("voice activity: {e:#}"));
                    Voice::energy(&audio)
                });
            analysis.speech = speech;
            let (levels, sound) = align::measure(&audio);
            let transcript = transcript(
                shared,
                task,
                stream,
                &audio,
                &analysis.speech,
                stretches.as_deref(),
                &progress,
            );
            match transcript {
                Ok(Some(mut t)) => {
                    align::refine(&mut t.words, &levels, &sound);
                    if let Err(e) = align_words(shared, &audio, &mut t, &progress) {
                        // The edges measured on the audio stand.
                        problems.push(format!("word alignment: {e:#}"));
                    }
                    // After the aligner, so each word is heard where it is.
                    if let Err(e) = attribute_speakers(shared, &audio, &mut t, &progress) {
                        problems.push(format!("speakers: {e:#}"));
                    }
                    analysis.transcript = Some(t);
                }
                Ok(None) => unpersisted = true,
                Err(e) => problems.push(format!("transcript: {e:#}")),
            }
            drop(audio);
            analysis.levels = Some(levels);
            if let Err(e) = hear(shared, task, stream, &mut analysis, &progress) {
                problems.push(format!("sounds: {e:#}"));
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
            Err(e) => problems.push(format!("shot changes: {e:#}")),
        }
    }

    let fp = &task.fingerprint;
    analysis.version = ANALYSIS_VERSION;
    let complete = problems.is_empty();
    if complete
        && !unpersisted
        && let Err(e) = cache::store(fp, &analysis)
    {
        problems.push(format!("cache: {e:#}"));
    }
    lock(&shared.results).insert(fp.clone(), Arc::new(analysis));
    if complete {
        lock(&shared.incomplete).remove(fp);
    } else {
        lock(&shared.incomplete).insert(fp.clone());
    }
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
/// in for it in a build without it.
#[cfg(feature = "whisper")]
fn listen(shared: &Shared, audio: &[f32], progress: &Progress) -> anyhow::Result<Voice> {
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
    }))
}

/// Sounds, music, beats and sound vectors for `analysis`, whose levels are
/// measured. A model that could not be had is an error; what the others
/// heard is kept.
#[cfg(feature = "sounds")]
fn hear(
    shared: &Shared,
    task: &Task,
    stream: u32,
    analysis: &mut Analysis,
    progress: &Progress,
) -> anyhow::Result<()> {
    let heard = shared.sounds.hear(
        &task.item.path,
        stream,
        task.item.info.as_ref().map(|i| i.duration),
        analysis.levels.as_ref(),
        &progress.step(Step::Sounds),
        &progress.step(Step::Beats),
    )?;
    analysis.sounds = heard.sounds;
    analysis.music = heard.music;
    let mut problems = heard.problems;
    if let Some(vectors) = heard.vectors {
        if let Err(e) = cache::store_vectors(&task.fingerprint, &vectors) {
            problems.push(format!("sound vectors: {e:#}"));
        }
        lock(&shared.vectors).insert(task.fingerprint.clone(), Arc::new(vectors));
    }
    if !problems.is_empty() {
        anyhow::bail!("{}", problems.join("; "));
    }
    Ok(())
}

#[cfg(not(feature = "sounds"))]
fn hear(
    _shared: &Shared,
    _task: &Task,
    _stream: u32,
    _analysis: &mut Analysis,
    _progress: &Progress,
) -> anyhow::Result<()> {
    Ok(())
}

/// Fit the transcript's words to the audio with the alignment model. A
/// language it does not know keeps the edges measured on the audio, for
/// good.
#[cfg(feature = "aligner")]
fn align_words(
    shared: &Shared,
    audio: &[f32],
    transcript: &mut Transcript,
    progress: &Progress,
) -> anyhow::Result<()> {
    if transcript.words.is_empty() || !shared.aligner.supports(&transcript.language) {
        return Ok(());
    }
    shared
        .aligner
        .align(audio, &mut transcript.words, &progress.step(Step::Words))
}

#[cfg(not(feature = "aligner"))]
fn align_words(
    _shared: &Shared,
    _audio: &[f32],
    _transcript: &mut Transcript,
    _progress: &Progress,
) -> anyhow::Result<()> {
    Ok(())
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
