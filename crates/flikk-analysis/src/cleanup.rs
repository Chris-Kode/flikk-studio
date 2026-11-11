//! Cleaned copies of source sound: [`LocalCleanup`] implements
//! `CleanupService`. A copy is the whole source stream decoded through the
//! media engine as the preview mixer hears it (stereo, at
//! [`cleanup::RATE`]), cleaned, and written as 24-bit FLAC under
//! `paths::cache()` by the name [`Treatment::file`] gives it, through a
//! temp file placed once whole. Noise Reduction is FFmpeg's `afftdn` on
//! that stream, its block delay taken out.
//!
//! Copies are made one at a time on a worker thread of their own, the
//! newest asked for first. One that fails is not tried again for
//! [`flikk_os::backoff::RETRY_AFTER`]; renders make theirs on their own
//! thread and try all the same.

use flikk_core::cache_sweep::mark_used;
use flikk_core::cleanup::{self, Denoise, Progress, Treatment};
use flikk_core::model::MediaItem;
use flikk_core::services::{CleanupService, MediaService, PcmStream, ServiceError, ServiceResult};
use flikk_core::time::Time;
use flikk_os::backoff::Failures;
use flikk_os::fs::{place_new, temp_sibling};
use flikk_os::process::{ChildGuard, command, ffmpeg};
use flikk_os::sync::{lock, panic_message, wait};
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

/// Stereo frames read and written at a time.
const BLOCK: usize = 48_000;

/// Makes and keeps cleaned copies of source streams.
pub struct LocalCleanup {
    shared: Arc<Shared>,
}

struct Shared {
    media: Arc<dyn MediaService>,
    state: Mutex<State>,
    wake: Condvar,
    version: AtomicU64,
}

/// A copy to make: of `stream` of `item`, by `treatment`, into `file`.
#[derive(Clone)]
struct Job {
    item: MediaItem,
    stream: u32,
    treatment: Treatment,
    file: PathBuf,
}

#[derive(Default)]
struct State {
    /// Waiting, the newest first.
    queue: VecDeque<Job>,
    /// Being made, by file, and how far.
    making: HashMap<PathBuf, f32>,
    failures: Failures<PathBuf>,
}

impl LocalCleanup {
    pub fn new(media: Arc<dyn MediaService>) -> Self {
        let shared = Arc::new(Shared {
            media,
            state: Mutex::default(),
            wake: Condvar::new(),
            version: AtomicU64::new(0),
        });
        let worker = shared.clone();
        let _ = std::thread::Builder::new()
            .name("flikk-cleanup".into())
            .spawn(move || worker.work());
        Self { shared }
    }
}

/// The file `treatment` keeps its copy of `stream` of `item` in, or why
/// it has none.
fn file_of(item: &MediaItem, stream: u32, treatment: &Treatment) -> Result<PathBuf, String> {
    let fingerprint = item
        .fingerprint
        .as_ref()
        .ok_or_else(|| format!("{} is not fingerprinted yet", item.path.display()))?;
    Ok(treatment.file(fingerprint, stream))
}

impl Shared {
    fn work(self: Arc<Self>) {
        loop {
            let job = {
                let mut state = lock(&self.state);
                loop {
                    match state.queue.pop_front() {
                        Some(job) => {
                            state.making.insert(job.file.clone(), 0.0);
                            break job;
                        }
                        None => state = wait(&self.wake, state),
                    }
                }
            };
            let file = job.file.clone();
            let progress = |p: f32| {
                if let Some(at) = lock(&self.state).making.get_mut(&file) {
                    *at = p;
                }
            };
            let never = AtomicBool::new(false);
            let made = catch_unwind(AssertUnwindSafe(|| {
                make(&*self.media, &job, &progress, &never)
            }))
            .unwrap_or_else(|panic| Err(ServiceError(panic_message("cleaning sound", &*panic))));
            let mut state = lock(&self.state);
            state.making.remove(&job.file);
            match made {
                Ok(()) => {
                    state.failures.clear(&job.file);
                }
                Err(e) => {
                    eprintln!("cleanup of {} failed: {e}", job.item.path.display());
                    state.failures.record(job.file.clone(), e.0);
                }
            }
            drop(state);
            self.version.fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl CleanupService for LocalCleanup {
    fn request(&self, item: &MediaItem, stream: u32, treatment: &Treatment) -> Progress {
        let file = match file_of(item, stream, treatment) {
            Ok(file) => file,
            Err(why) => return Progress::Unavailable(why),
        };
        if file.is_file() {
            mark_used(&file);
            return Progress::Made(file);
        }
        let mut state = lock(&self.shared.state);
        if let Some(at) = state.making.get(&file) {
            return Progress::Making(*at);
        }
        if let Some(why) = state.failures.recent(&file) {
            return Progress::Failed(why.to_string());
        }
        // Asked for again: to the front.
        state.queue.retain(|job| job.file != file);
        state.queue.push_front(Job {
            item: item.clone(),
            stream,
            treatment: treatment.clone(),
            file,
        });
        drop(state);
        self.shared.wake.notify_one();
        Progress::Making(0.0)
    }

    fn complete(
        &self,
        item: &MediaItem,
        stream: u32,
        treatment: &Treatment,
        progress: &dyn Fn(f32),
        cancel: &AtomicBool,
    ) -> ServiceResult<PathBuf> {
        let file = file_of(item, stream, treatment).map_err(ServiceError)?;
        if !file.is_file() {
            let job = Job {
                item: item.clone(),
                stream,
                treatment: treatment.clone(),
                file: file.clone(),
            };
            make(&*self.shared.media, &job, progress, cancel)?;
            self.shared.version.fetch_add(1, Ordering::Relaxed);
        }
        mark_used(&file);
        progress(1.0);
        Ok(file)
    }

    fn version(&self) -> u64 {
        self.shared.version.load(Ordering::Relaxed)
    }
}

/// Make `job`'s copy: decode, clean, encode into a temp file, then place
/// it, unless one was placed meanwhile.
fn make(
    media: &dyn MediaService,
    job: &Job,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> ServiceResult<()> {
    let fail = |what: &str, e: &dyn std::fmt::Display| ServiceError(format!("{what}: {e}"));
    if let Some(dir) = job.file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| fail("could not make the cache folder", &e))?;
    }
    let part = temp_sibling(&job.file);
    let result = write(media, job, &part, progress, cancel);
    let placed = result.and_then(|()| {
        place_new(&part, &job.file)
            .map(|_| ())
            .map_err(|e| fail("could not keep the cleaned sound", &e))
    });
    let _ = std::fs::remove_file(&part);
    placed
}

/// The stream decoded, cleaned and encoded into `part`.
fn write(
    media: &dyn MediaService,
    job: &Job,
    part: &Path,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> ServiceResult<()> {
    let Treatment::Denoise(settings) = &job.treatment;
    let mut source = media.audio(&job.item, job.stream, Time::ZERO, cleanup::RATE)?;
    let mut encoder = Encoder::start(part, &denoise_filters(settings))?;
    let total = job
        .item
        .info
        .as_ref()
        .map_or(0.0, |i| i.duration.as_secs_f64() * cleanup::RATE as f64);
    let mut block = vec![0f32; BLOCK * 2];
    let mut done = 0usize;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(ServiceError("cancelled".into()));
        }
        let n = read_full(&mut *source, &mut block)?;
        if n == 0 {
            break;
        }
        encoder.write(&block[..n])?;
        done += n / 2;
        if total > 0.0 {
            progress((done as f64 / total).min(0.99) as f32);
        }
    }
    encoder.finish()
}

/// Fill `out` as far as the stream goes; the samples read.
fn read_full(source: &mut dyn PcmStream, out: &mut [f32]) -> ServiceResult<usize> {
    let mut filled = 0;
    while filled < out.len() {
        match source.read(&mut out[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

/// `afftdn` at each of `settings` in turn, each one's block delay taken
/// out: the end padded, as much dropped from the start.
fn denoise_filters(settings: &[Denoise]) -> String {
    let delay = (afftdn_latency() * cleanup::RATE as f64).round() as i64;
    let chain: Vec<String> = settings
        .iter()
        .map(|d| {
            let afftdn = format!(
                "afftdn=nr={:.3}:nf={:.3}",
                d.reduction_db.clamp(0.01, 97.0),
                d.floor_db.clamp(-80.0, -20.0)
            );
            if delay > 0 {
                format!(
                    "apad=pad_len={delay},{afftdn},atrim=start_sample={delay},asetpts=PTS-STARTPTS"
                )
            } else {
                afftdn
            }
        })
        .collect();
    chain.join(",")
}

/// Output delay of `afftdn` in seconds, measured once on this FFmpeg:
/// noise through it next to the original, best-correlating lag. FFmpeg 6
/// delays by its 25 ms analysis window and has no option to compensate.
fn afftdn_latency() -> f64 {
    static LATENCY: OnceLock<f64> = OnceLock::new();
    *LATENCY.get_or_init(|| {
        const SR: usize = 48_000;
        let out = command(ffmpeg())
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "anoisesrc=d=0.5:r=48000:a=0.3:seed=3,asplit[a][b];[b]afftdn=nr=0.01:nf=-80[c];[a][c]amerge=inputs=2",
                "-f",
                "f32le",
                "-",
            ])
            .output();
        let Ok(out) = out else {
            return 0.025;
        };
        let v: Vec<f32> = out
            .stdout
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        let (input, output): (Vec<f32>, Vec<f32>) =
            v.as_chunks::<2>().0.iter().map(|f| (f[0], f[1])).unzip();
        let (from, len, max_lag) = (2_000, 8_000, SR / 10);
        if input.len() < from + len + max_lag {
            return 0.025;
        }
        let best = (0..max_lag)
            .map(|lag| {
                let c: f32 = (0..len)
                    .map(|i| input[from + i] * output[from + lag + i])
                    .sum();
                (lag, c)
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map_or(0, |(lag, _)| lag);
        best as f64 / SR as f64
    })
}

/// FFmpeg writing stereo [`cleanup::RATE`] audio from its stdin, through
/// `filters`, as 24-bit FLAC.
struct Encoder {
    child: ChildGuard,
    stdin: Option<std::process::ChildStdin>,
    bytes: Vec<u8>,
}

impl Encoder {
    fn start(out: &Path, filters: &str) -> ServiceResult<Self> {
        let rate = cleanup::RATE.to_string();
        let mut cmd = command(ffmpeg());
        cmd.args(["-v", "error", "-y", "-f", "f32le", "-ar", &rate, "-ac", "2"])
            .args(["-i", "pipe:0"]);
        if !filters.is_empty() {
            cmd.args(["-af", filters]);
        }
        cmd.args(["-c:a", "flac", "-sample_fmt", "s32", "-f", "flac"])
            .arg(out)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = ChildGuard::spawn(&mut cmd)
            .map_err(|e| ServiceError(format!("could not run ffmpeg: {e}")))?;
        let stdin = child.stdin.take();
        Ok(Self {
            child,
            stdin,
            bytes: Vec::new(),
        })
    }

    fn write(&mut self, samples: &[f32]) -> ServiceResult<()> {
        self.bytes.clear();
        self.bytes
            .extend(samples.iter().flat_map(|s| s.to_le_bytes()));
        let stdin = self.stdin.as_mut().expect("open until finished");
        stdin
            .write_all(&self.bytes)
            .map_err(|e| ServiceError(format!("could not write the cleaned sound: {e}")))
    }

    fn finish(mut self) -> ServiceResult<()> {
        drop(self.stdin.take());
        let mut stderr = String::new();
        if let Some(mut pipe) = self.child.stderr.take() {
            let _ = std::io::Read::read_to_string(&mut pipe, &mut stderr);
        }
        let status = self
            .child
            .wait()
            .map_err(|e| ServiceError(format!("ffmpeg: {e}")))?;
        if status.success() {
            Ok(())
        } else {
            let last = stderr.lines().rev().find(|l| !l.trim().is_empty());
            Err(ServiceError(format!(
                "ffmpeg could not write the cleaned sound: {}",
                last.unwrap_or("unknown error").trim()
            )))
        }
    }
}
