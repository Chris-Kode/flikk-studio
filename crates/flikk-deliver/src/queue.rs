//! Render queue (PRD D1): jobs run one after another on a background
//! thread, report progress from `-progress pipe:1`, can be canceled, and
//! never overwrite an existing file.

use crate::composed::{self, Frames};
use crate::ffmpeg::{self, TempOutput, ensure_free, filter_script_args, publish};
use crate::graph::{self, GraphSpec, Loudnorm, Measured};
use crate::hw::{self, Encoder, HwEncoder};
use crate::preset::{LoudnessTarget, RenderSettings};
use anyhow::{Context as _, Result, anyhow};
use flikk_core::analysis::Loudness;
use flikk_core::memory::CaptionStyle;
use flikk_core::model::Project;
use flikk_core::paths;
use flikk_core::services::Services;
use flikk_core::{JobId, TimeRange};
use flikk_os::sync::{lock, panic_message};
use std::cell::Cell;
use std::ffi::OsString;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

/// Everything a render needs, owned so it can cross threads.
#[derive(Clone, Debug)]
pub struct RenderRequest {
    pub project: Project,
    pub settings: RenderSettings,
    pub output: PathBuf,
    /// The style profile's caption presets, for burn-in.
    pub caption_styles: Vec<CaptionStyle>,
    /// Engines for the compositor (decode, effects, HTML); ranges that
    /// need it are composited frame by frame.
    pub services: Services,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RenderState {
    Queued,
    Running { fraction: f32 },
    Done,
    Failed { message: String },
    Canceled,
}

impl RenderState {
    pub fn is_finished(&self) -> bool {
        matches!(
            self,
            RenderState::Done | RenderState::Failed { .. } | RenderState::Canceled
        )
    }
}

#[derive(Clone, Debug)]
pub struct RenderJob {
    pub id: JobId,
    /// Preset name.
    pub label: String,
    /// The file asked for; once done, the file written (the next free name
    /// when one appeared at the asked path during the render).
    pub output: PathBuf,
    pub state: RenderState,
}

struct Entry {
    job: RenderJob,
    request: Option<RenderRequest>,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
struct Inner {
    entries: Mutex<Vec<Entry>>,
    worker: AtomicBool,
}

impl Inner {
    /// The entries, even after a panic poisoned the lock: every write to
    /// them is a single field store, so they are never left half-changed.
    fn entries(&self) -> MutexGuard<'_, Vec<Entry>> {
        lock(&self.entries)
    }
}

/// Clears `worker` if the worker thread ends some way other than finding
/// the queue empty, so the next `enqueue` starts a new one.
struct WorkerGuard<'a>(&'a Inner);

impl Drop for WorkerGuard<'_> {
    fn drop(&mut self) {
        self.0.worker.store(false, Ordering::Release);
    }
}

/// Every queue alive, so the outputs of pending jobs are kept from every
/// other output.
static QUEUES: Mutex<Vec<Weak<Inner>>> = Mutex::new(Vec::new());

/// Held while an output name is checked and then taken, by a job queued
/// for it or a file published to it, so two never take the same name.
static CLAIMING: Mutex<()> = Mutex::new(());

pub(crate) fn claiming() -> MutexGuard<'static, ()> {
    lock(&CLAIMING)
}

/// Whether `path` is the output of a job still queued or running, other
/// than `except`, in any queue. Never call it holding a queue's entries.
pub(crate) fn claimed(path: &Path, except: Option<JobId>) -> bool {
    let mut queues = lock(&QUEUES);
    queues.retain(|q| q.strong_count() > 0);
    queues.iter().filter_map(Weak::upgrade).any(|inner| {
        inner
            .entries()
            .iter()
            .any(|e| Some(e.job.id) != except && !e.job.state.is_finished() && e.job.output == path)
    })
}

/// Cheap to clone; all clones share one queue.
#[derive(Clone)]
pub struct RenderQueue {
    inner: Arc<Inner>,
}

impl Default for RenderQueue {
    fn default() -> Self {
        let inner = Arc::new(Inner::default());
        let mut queues = lock(&QUEUES);
        queues.push(Arc::downgrade(&inner));
        Self { inner }
    }
}

impl RenderQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue a render. Fails at once if the output exists or is already the
    /// target of a queued job.
    pub fn enqueue(&self, request: RenderRequest) -> Result<JobId> {
        let claiming = claiming();
        ensure_free(&request.output)?;
        if claimed(&request.output, None) {
            return Err(anyhow!(
                "{} is already in the queue",
                request.output.display()
            ));
        }
        let mut entries = self.inner.entries();
        let id = JobId::new();
        entries.push(Entry {
            job: RenderJob {
                id,
                label: request.settings.name.clone(),
                output: request.output.clone(),
                state: RenderState::Queued,
            },
            request: Some(request),
            cancel: Arc::new(AtomicBool::new(false)),
        });
        drop(entries);
        drop(claiming);
        self.ensure_worker();
        Ok(id)
    }

    pub fn cancel(&self, id: JobId) {
        let mut entries = self.inner.entries();
        if let Some(e) = entries.iter_mut().find(|e| e.job.id == id) {
            e.cancel.store(true, Ordering::Relaxed);
            if e.job.state == RenderState::Queued {
                e.job.state = RenderState::Canceled;
                e.request = None;
            }
        }
    }

    /// Cancel every job and wait (a few seconds at most) for the running
    /// one to stop and remove its partial file. For quitting: a render cut
    /// off by the app's exit leaves its temp file in the output folder.
    pub fn shutdown(&self) {
        let ids: Vec<JobId> = self
            .jobs()
            .into_iter()
            .filter(|j| !j.state.is_finished())
            .map(|j| j.id)
            .collect();
        for id in ids {
            self.cancel(id);
        }
        let deadline = Instant::now() + SHUTDOWN_WAIT;
        while self.is_busy() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Snapshot of every job, oldest first.
    pub fn jobs(&self) -> Vec<RenderJob> {
        let entries = self.inner.entries();
        entries.iter().map(|e| e.job.clone()).collect()
    }

    pub fn is_busy(&self) -> bool {
        self.jobs().iter().any(|j| !j.state.is_finished())
    }

    /// Forget one finished job.
    pub fn remove(&self, id: JobId) {
        let mut entries = self.inner.entries();
        entries.retain(|e| e.job.id != id || !e.job.state.is_finished());
    }

    /// Forget finished jobs.
    pub fn clear_finished(&self) {
        let mut entries = self.inner.entries();
        entries.retain(|e| !e.job.state.is_finished());
    }

    fn set_state(&self, id: JobId, state: RenderState, output: Option<PathBuf>) {
        let mut entries = self.inner.entries();
        if let Some(e) = entries.iter_mut().find(|e| e.job.id == id) {
            e.job.state = state;
            if let Some(output) = output {
                e.job.output = output;
            }
        }
    }

    /// Progress only moves forward and stays within 0..1, whatever the
    /// render's phases report.
    fn set_progress(&self, id: JobId, fraction: f32) {
        let mut entries = self.inner.entries();
        if let Some(e) = entries.iter_mut().find(|e| e.job.id == id)
            && let RenderState::Running { fraction: shown } = &mut e.job.state
        {
            *shown = shown.max(fraction.clamp(0.0, 1.0));
        }
    }

    fn ensure_worker(&self) {
        if self.inner.worker.swap(true, Ordering::AcqRel) {
            return;
        }
        let queue = self.clone();
        let spawned = std::thread::Builder::new()
            .name("flikk-render".into())
            .spawn(move || queue.work());
        if let Err(err) = spawned {
            // Nothing will pick the waiting jobs up: fail them rather than
            // leave them queued forever.
            let mut entries = self.inner.entries();
            for e in entries.iter_mut() {
                if e.job.state == RenderState::Queued && e.request.take().is_some() {
                    e.job.state = RenderState::Failed {
                        message: format!("could not start rendering: {err}"),
                    };
                }
            }
            self.inner.worker.store(false, Ordering::Release);
        }
    }

    /// Run queued jobs until there are none. A job that panics fails, and
    /// the next one runs.
    fn work(&self) {
        let guard = WorkerGuard(&self.inner);
        loop {
            let next = {
                let mut entries = self.inner.entries();
                let next = entries
                    .iter_mut()
                    .find(|e| e.job.state == RenderState::Queued && e.request.is_some());
                match next {
                    Some(e) => {
                        e.job.state = RenderState::Running { fraction: 0.0 };
                        (
                            e.job.id,
                            e.request.take().expect("queued"),
                            e.cancel.clone(),
                        )
                    }
                    None => {
                        // Cleared under the lock, so a job queued from now
                        // on starts a new worker.
                        self.inner.worker.store(false, Ordering::Release);
                        std::mem::forget(guard);
                        return;
                    }
                }
            };
            let (id, request, cancel) = next;
            let progress = |fraction: f32| self.set_progress(id, fraction);
            let result = catch_unwind(AssertUnwindSafe(|| {
                render(&request, Some(id), &progress, &cancel)
            }));
            let written = result.as_ref().ok().and_then(|r| r.as_ref().ok()).cloned();
            let state = match result {
                Ok(Ok(_)) => RenderState::Done,
                Ok(Err(_)) if cancel.load(Ordering::Relaxed) => RenderState::Canceled,
                Ok(Err(e)) => RenderState::Failed {
                    message: format!("{e:#}"),
                },
                Err(payload) => RenderState::Failed {
                    message: panic_message("the render", payload.as_ref()),
                },
            };
            self.set_state(id, state, written);
        }
    }
}

/// Scratch directory for one render's script and side files, removed on
/// drop.
pub(crate) struct WorkDir(pub PathBuf);

impl WorkDir {
    pub(crate) fn new() -> Result<Self> {
        let dir = paths::cache()
            .join("render")
            .join(JobId::new().0.simple().to_string());
        std::fs::create_dir_all(&dir).context("could not create the render cache folder")?;
        Ok(Self(dir))
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(crate) fn write_script(dir: &Path, name: &str, script: &str) -> Result<PathBuf> {
    let path = dir.join(name);
    std::fs::write(&path, script).context("could not write the filter script")?;
    Ok(path)
}

/// The part of a render's progress spent finding subject masks, when it
/// has any to find (tracking dwarfs the encode).
const SUBJECTS_SHARE: f32 = 0.5;
/// Of the progress, what measuring stabilized clips' camera takes.
const MOTION_SHARE: f32 = 0.2;
/// The part of what is left spent making depth maps, when it has any to
/// make.
const DEPTH_SHARE: f32 = 0.5;
/// The part of what is left spent measuring loudness, when it is
/// normalized.
const LOUDNESS_SHARE: f32 = 0.2;

/// How long [`RenderQueue::shutdown`] waits for the running job to stop.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(3);

/// Render synchronously on the calling thread. `progress` gets 0..1.
pub fn render_blocking(
    request: &RenderRequest,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> Result<()> {
    render(request, None, progress, cancel).map(|_| ())
}

/// The loudness of the mix `spec` would render, as `loudnorm` measures it
/// against `target`; `work` holds the filter script.
fn measure(
    project: &Project,
    spec: &GraphSpec,
    target: LoudnessTarget,
    work: &Path,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> Result<Measured> {
    let mut measure = spec.clone();
    measure.video = false;
    measure.loudnorm = Some(Loudnorm::Measure(target));
    let g = graph::build(project, &measure, work)?;
    let script = write_script(work, "measure.txt", &g.script)?;
    let mut args: Vec<OsString> = vec!["-loglevel".into(), "info".into()];
    args.extend(filter_script_args(&script));
    args.extend(["-map", "[aout]", "-f", "null", "-"].map(OsString::from));
    let stderr = ffmpeg::run(&args, g.seconds, progress, cancel)?;
    Measured::parse(&stderr).ok_or_else(|| anyhow!("could not measure loudness (silent mix?)"))
}

/// The programme loudness and true peak of `project`'s mix over `range`
/// (the whole timeline when `None`), as a render would make it before any
/// normalization. Blocks while FFmpeg plays the mix through; `progress`
/// gets 0..1, and `cancel` stops it with an error.
pub fn measure_mix(
    project: &Project,
    range: Option<TimeRange>,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> Result<Loudness> {
    let settings = RenderSettings {
        range,
        ..RenderSettings::wav()
    };
    let spec = GraphSpec::new(project, &settings, &[]);
    let work = WorkDir::new()?;
    // The measurement doesn't depend on the target.
    let m = measure(
        project,
        &spec,
        LoudnessTarget::default(),
        &work.0,
        progress,
        cancel,
    )?;
    let number = |v: &str| v.parse::<f32>().context("loudness out of range");
    Ok(Loudness {
        integrated_lufs: number(&m.input_i)?,
        true_peak_db: number(&m.input_tp)?,
        range_lu: number(&m.input_lra)?,
    })
}

/// [`render_blocking`], returning the path written: `request.output`, or
/// the next name free of files and of other jobs' outputs when a file
/// appeared there during the render. `job` is the queued job rendering.
fn render(
    request: &RenderRequest,
    job: Option<JobId>,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> Result<PathBuf> {
    let out = &request.output;
    ensure_free(out)?;
    let work = WorkDir::new()?;
    let settings = &request.settings;
    let mut spec = GraphSpec::new(&request.project, settings, &request.caption_styles);
    let mut share = 0.0;

    // Subject and person masks: found (and followed) before any frame is
    // composed.
    if spec.video && composed::wanted(&request.project, spec.range, &request.services) {
        let found = composed::find_subjects(
            &request.project,
            spec.range,
            &request.services,
            &|p| progress(p * SUBJECTS_SHARE),
            cancel,
        )?;
        if found {
            share = SUBJECTS_SHARE;
        }
        // Stabilized clips: their camera paths, measured first too.
        let from = share;
        let measured = composed::measure_motion(
            &request.project,
            spec.range,
            &request.services,
            &|p| progress(from + p * MOTION_SHARE),
            cancel,
        )?;
        if measured {
            share += MOTION_SHARE;
        }
        // Depth maps too.
        let (from, part) = (share, (1.0 - share) * DEPTH_SHARE);
        let made = composed::find_depth(
            &request.project,
            spec.range,
            &request.services,
            &|p| progress(from + p * part),
            cancel,
        )?;
        if made {
            share = from + part;
        }
    }

    // Pass 1 of 2: measure loudness of the final mix.
    if let Some(target) = settings.loudness {
        let from = share;
        share += (1.0 - share) * LOUDNESS_SHARE;
        let measured = measure(
            &request.project,
            &spec,
            target,
            &work.0,
            &|p| progress(from + p * (share - from)),
            cancel,
        )?;
        spec.loudnorm = Some(Loudnorm::Apply(target, measured));
    }

    let composite = composed::wanted(&request.project, spec.range, &request.services);
    composed::route(&request.project, &mut spec, &request.services, composite);
    let g = graph::build(&request.project, &spec, &work.0)?;
    // Removed however the render ends, once published or not.
    let tmp = TempOutput::new(out);
    let progress = |p: f32| progress(share + p * (1.0 - share));
    let attempts = attempts(settings, &spec, cancel)?;
    let last = attempts.len() - 1;
    // How far the encode got, so a fallback carries on from there rather
    // than start the bar over.
    let reached = Cell::new(0.0f32);
    for (i, hw) in attempts.iter().enumerate() {
        let from = reached.get();
        let progress = |p: f32| {
            let p = from + p.clamp(0.0, 1.0) * (1.0 - from);
            reached.set(reached.get().max(p));
            progress(p);
        };
        let result = encode(
            request,
            &spec,
            &g,
            &work.0,
            hw.as_ref(),
            tmp.path(),
            &progress,
            cancel,
        );
        match result {
            Ok(()) => {
                // Software worked where the hardware before it failed: skip
                // that hardware for this size from now on.
                if i > 0 {
                    let mut failed = lock(&FAILED);
                    for hw in attempts[..i].iter().flatten() {
                        failed.push((hw.clone(), spec.width, spec.height));
                    }
                }
                return publish(&tmp, out, job);
            }
            Err(e) => {
                // The next attempt starts from nothing.
                let _ = std::fs::remove_file(tmp.path());
                // Auto: a hardware encoder that fails hands over to software.
                if i == last || cancel.load(Ordering::Relaxed) {
                    return Err(match hw {
                        Some(hw) => e.context(format!("{} encoder", hw.api.label())),
                        None => e,
                    });
                }
            }
        }
    }
    unreachable!("at least one attempt")
}

/// Hardware encoders that failed a render software then finished, with
/// the size they failed at, for this session.
static FAILED: Mutex<Vec<(HwEncoder, u32, u32)>> = Mutex::new(Vec::new());

/// The encoders to try in turn: `None` is software. Waits for the
/// hardware probe, unless `cancel` is set meanwhile.
fn attempts(
    settings: &RenderSettings,
    spec: &GraphSpec,
    cancel: &AtomicBool,
) -> Result<Vec<Option<HwEncoder>>> {
    let Some((codec, _)) = settings.hardware_codec().filter(|_| spec.video) else {
        return Ok(vec![None]);
    };
    if settings.encoder == Encoder::Software {
        return Ok(vec![None]);
    }
    let probe = hw::probe_until(cancel).ok_or_else(|| anyhow!("canceled"))?;
    let best = probe.best(codec).cloned();
    Ok(match settings.encoder {
        Encoder::Software => vec![None],
        Encoder::Hardware => {
            vec![Some(best.ok_or_else(|| {
                anyhow!("no hardware encoder for this format on this machine")
            })?)]
        }
        Encoder::Auto => {
            let failed = lock(&FAILED);
            let size = (spec.width, spec.height);
            match best.filter(|hw| !failed.iter().any(|(f, w, h)| f == hw && (*w, *h) == size)) {
                Some(hw) => vec![Some(hw), None],
                None => vec![None],
            }
        }
    })
}

/// One run of the final encode into `tmp`.
#[allow(clippy::too_many_arguments)]
fn encode(
    request: &RenderRequest,
    spec: &GraphSpec,
    g: &graph::Graph,
    dir: &Path,
    hw: Option<&HwEncoder>,
    tmp: &Path,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> Result<()> {
    let settings = &request.settings;
    let mut script = g.script.clone();
    let mut video = g.video.clone();
    if let (Some(v), Some(upload)) = (&video, hw.and_then(HwEncoder::upload_filter)) {
        script = format!("{};\n[{v}]{upload}[vhw]\n", script.trim_end());
        video = Some("vhw".into());
    }
    let script = write_script(dir, "graph.txt", &script)?;
    let mut args: Vec<OsString> = vec!["-loglevel".into(), "error".into(), "-y".into()];
    if let Some(hw) = hw {
        args.extend(hw.global_args());
    }
    if spec.piped {
        args.extend(composed::input_args(spec));
    }
    args.extend(filter_script_args(&script));
    if let Some(v) = &video {
        args.extend(["-map".into(), format!("[{v}]").into()]);
    }
    if let Some(a) = &g.audio {
        args.extend(["-map".into(), format!("[{a}]").into()]);
    }
    let codec = match hw {
        Some(hw) => settings.hw_codec_args(hw, spec.width, spec.height, spec.rate.fps()),
        None => settings.codec_args(),
    };
    args.extend(codec.into_iter().map(OsString::from));
    args.push(tmp.into());
    if spec.piped {
        let mut frames = Frames::new(&request.project, &request.services, spec);
        composed::run(&args, g, &mut frames, progress, cancel)
    } else {
        ffmpeg::run(&args, g.seconds, progress, cancel).map(|_| ())
    }
}
