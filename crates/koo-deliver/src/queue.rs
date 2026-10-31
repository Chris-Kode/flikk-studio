//! Render queue (PRD D1): jobs run one after another on a background
//! thread, report progress from `-progress pipe:1`, can be canceled, and
//! never overwrite an existing file.

use crate::composed::{self, Frames};
use crate::ffmpeg::{self, ensure_free, filter_script_args, publish, temp_path};
use crate::graph::{self, GraphSpec, Loudnorm, Measured};
use crate::preset::RenderSettings;
use anyhow::{Context as _, Result, anyhow};
use koo_core::JobId;
use koo_core::memory::CaptionStyle;
use koo_core::model::Project;
use koo_core::paths;
use koo_core::services::Services;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

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

/// Cheap to clone; all clones share one queue.
#[derive(Clone, Default)]
pub struct RenderQueue {
    inner: Arc<Inner>,
}

impl RenderQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue a render. Fails at once if the output exists or is already the
    /// target of a queued job.
    pub fn enqueue(&self, request: RenderRequest) -> Result<JobId> {
        ensure_free(&request.output)?;
        let mut entries = self.inner.entries.lock().expect("queue lock");
        if entries
            .iter()
            .any(|e| !e.job.state.is_finished() && e.job.output == request.output)
        {
            return Err(anyhow!(
                "{} is already in the queue",
                request.output.display()
            ));
        }
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
        self.ensure_worker();
        Ok(id)
    }

    pub fn cancel(&self, id: JobId) {
        let mut entries = self.inner.entries.lock().expect("queue lock");
        if let Some(e) = entries.iter_mut().find(|e| e.job.id == id) {
            e.cancel.store(true, Ordering::Relaxed);
            if e.job.state == RenderState::Queued {
                e.job.state = RenderState::Canceled;
                e.request = None;
            }
        }
    }

    /// Snapshot of every job, oldest first.
    pub fn jobs(&self) -> Vec<RenderJob> {
        let entries = self.inner.entries.lock().expect("queue lock");
        entries.iter().map(|e| e.job.clone()).collect()
    }

    pub fn is_busy(&self) -> bool {
        self.jobs().iter().any(|j| !j.state.is_finished())
    }

    /// Forget one finished job.
    pub fn remove(&self, id: JobId) {
        let mut entries = self.inner.entries.lock().expect("queue lock");
        entries.retain(|e| e.job.id != id || !e.job.state.is_finished());
    }

    /// Forget finished jobs.
    pub fn clear_finished(&self) {
        let mut entries = self.inner.entries.lock().expect("queue lock");
        entries.retain(|e| !e.job.state.is_finished());
    }

    fn set_state(&self, id: JobId, state: RenderState) {
        let mut entries = self.inner.entries.lock().expect("queue lock");
        if let Some(e) = entries.iter_mut().find(|e| e.job.id == id) {
            e.job.state = state;
        }
    }

    fn ensure_worker(&self) {
        if self.inner.worker.swap(true, Ordering::AcqRel) {
            return;
        }
        let queue = self.clone();
        std::thread::Builder::new()
            .name("koo-render".into())
            .spawn(move || queue.work())
            .expect("spawn render worker");
    }

    fn work(&self) {
        loop {
            let next = {
                let mut entries = self.inner.entries.lock().expect("queue lock");
                let next = entries
                    .iter_mut()
                    .find(|e| e.job.state == RenderState::Queued && e.request.is_some());
                match next {
                    Some(e) => {
                        e.job.state = RenderState::Running { fraction: 0.0 };
                        Some((
                            e.job.id,
                            e.request.take().expect("queued"),
                            e.cancel.clone(),
                        ))
                    }
                    None => {
                        self.inner.worker.store(false, Ordering::Release);
                        None
                    }
                }
            };
            let Some((id, request, cancel)) = next else {
                return;
            };
            let progress = |fraction: f32| self.set_state(id, RenderState::Running { fraction });
            let state = match render_blocking(&request, &progress, &cancel) {
                Ok(()) => RenderState::Done,
                Err(_) if cancel.load(Ordering::Relaxed) => RenderState::Canceled,
                Err(e) => RenderState::Failed {
                    message: format!("{e:#}"),
                },
            };
            self.set_state(id, state);
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

/// Render synchronously on the calling thread. `progress` gets 0..1.
pub fn render_blocking(
    request: &RenderRequest,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> Result<()> {
    let out = &request.output;
    ensure_free(out)?;
    let work = WorkDir::new()?;
    let settings = &request.settings;
    let mut spec = GraphSpec::new(&request.project, settings, &request.caption_styles);
    let mut share = 0.0;

    // Subject masks: found (and followed) before any frame is composed.
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
    }

    // Pass 1 of 2: measure loudness of the final mix.
    if let Some(target) = settings.loudness {
        let mut measure = spec.clone();
        measure.video = false;
        measure.loudnorm = Some(Loudnorm::Measure(target));
        let g = graph::build(&request.project, &measure, &work.0)?;
        let script = write_script(&work.0, "measure.txt", &g.script)?;
        let mut args: Vec<OsString> = vec!["-loglevel".into(), "info".into()];
        args.extend(filter_script_args(&script));
        args.extend(["-map", "[aout]", "-f", "null", "-"].map(OsString::from));
        let from = share;
        share += 0.2;
        let stderr = ffmpeg::run(
            &args,
            g.seconds,
            &|p| progress(from + p * (share - from)),
            cancel,
        )?;
        let measured = Measured::parse(&stderr)
            .ok_or_else(|| anyhow!("could not measure loudness (silent mix?)"))?;
        spec.loudnorm = Some(Loudnorm::Apply(target, measured));
    }

    spec.piped = spec.video && composed::wanted(&request.project, spec.range, &request.services);
    let g = graph::build(&request.project, &spec, &work.0)?;
    let script = write_script(&work.0, "graph.txt", &g.script)?;
    let tmp = temp_path(out, &JobId::new().to_string());
    let mut args: Vec<OsString> = vec!["-loglevel".into(), "error".into(), "-y".into()];
    if spec.piped {
        args.extend(composed::input_args(&spec));
    }
    args.extend(filter_script_args(&script));
    if let Some(v) = &g.video {
        args.extend(["-map".into(), format!("[{v}]").into()]);
    }
    if let Some(a) = &g.audio {
        args.extend(["-map".into(), format!("[{a}]").into()]);
    }
    args.extend(settings.codec_args().into_iter().map(OsString::from));
    args.push(tmp.clone().into());
    let progress = |p: f32| progress(share + p * (1.0 - share));
    let result = if spec.piped {
        let mut frames = Frames::new(&request.project, &request.services, &spec);
        composed::run(&args, &g, &mut frames, &progress, cancel)
    } else {
        ffmpeg::run(&args, g.seconds, &progress, cancel).map(|_| ())
    };
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    publish(&tmp, out)
}
