//! Optical flow slow motion: [`LocalInterpolation`] implements
//! `InterpolationService` on the frame interpolation model (RIFE,
//! `koo-models`).
//!
//! A frame between two source frames is kept by the media's fingerprint
//! (its path without one), the two frames' source times, the moment
//! between them, its size and the model: in memory (the last
//! [`KEPT`]) and under `paths::cache()/interpolated/<fp>/`, raw RGBA, so a
//! frame is made once. The viewer asks without waiting: frames are made on
//! one thread of their own, the newest asked first (the playhead moved
//! on), and blended until they are made. Renders wait for each. A frame
//! that fails is not tried again until the model is downloaded anew.

use koo_core::paths;
use koo_core::services::{Between, Frame, InterpolationService};
use koo_core::subject::Fnv;
use koo_models::Task as ModelTask;
use koo_models::interpolation::Interpolator;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Read as _, Write as _};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

/// Frames kept in memory.
const KEPT: usize = 48;
/// Frames waiting to be made; older requests go first.
const WAITING: usize = 24;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The [`InterpolationService`] that runs the model on this machine.
pub struct LocalInterpolation {
    shared: Arc<Shared>,
}

struct Shared {
    runner: Mutex<Option<(&'static str, Arc<dyn Interpolator>)>>,
    made: Mutex<Made>,
    /// Requests not made yet, newest last.
    waiting: Mutex<VecDeque<Job>>,
    wake: Condvar,
    /// Keys that failed, and whether the model was there then.
    failed: Mutex<HashMap<u64, bool>>,
    version: AtomicU64,
}

#[derive(Default)]
struct Made {
    frames: HashMap<u64, Frame>,
    order: VecDeque<u64>,
    /// Keys looked for on disk and not found.
    missing: HashSet<u64>,
}

struct Job {
    key: u64,
    folder: String,
    a: Frame,
    b: Frame,
    at: f32,
}

impl Default for LocalInterpolation {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalInterpolation {
    pub fn new() -> Self {
        let shared = Arc::new(Shared {
            runner: Mutex::new(None),
            made: Mutex::default(),
            waiting: Mutex::default(),
            wake: Condvar::new(),
            failed: Mutex::default(),
            version: AtomicU64::new(0),
        });
        let worker = shared.clone();
        let _ = std::thread::Builder::new()
            .name("koo-interpolate".into())
            .spawn(move || worker.work());
        Self { shared }
    }

    /// Whether this build makes optical flow frames (the `interpolation`
    /// feature).
    pub fn in_build() -> bool {
        cfg!(feature = "interpolation")
    }
}

fn runner(model: &'static koo_models::Model) -> Option<Arc<dyn Interpolator>> {
    #[cfg(feature = "interpolation")]
    {
        Some(Arc::from(koo_models::interpolation::runner(model)))
    }
    #[cfg(not(feature = "interpolation"))]
    {
        let _ = model;
        None
    }
}

/// What a request asks of the picked model, as a number, and the folder
/// its frames are kept in.
fn key(request: &Between<'_>) -> (u64, String) {
    let model = ModelTask::FrameInterpolation.model();
    let folder = match &request.item.fingerprint {
        Some(fp) => fp.0.clone(),
        None => {
            let mut h = Fnv::default();
            h.write(request.item.path.to_string_lossy().as_bytes());
            format!("p{:016x}", h.0)
        }
    };
    let mut h = Fnv::default();
    h.write(folder.as_bytes());
    h.write(model.id.as_bytes());
    h.write(&request.a.0.0.to_le_bytes());
    h.write(&request.b.0.0.to_le_bytes());
    h.write(&request.at.to_bits().to_le_bytes());
    h.write(&request.a.1.width.to_le_bytes());
    h.write(&request.a.1.height.to_le_bytes());
    // HDR frames look as their tone map makes them (`koo_core::hdr`).
    if let Some(mapping) = koo_core::hdr::Mapping::of(request.item) {
        h.write(mapping.key().as_bytes());
    }
    (h.0, folder)
}

fn file(folder: &str, key: u64) -> PathBuf {
    paths::cache()
        .join("interpolated")
        .join(folder)
        .join(format!("{key:016x}.rgba"))
}

/// A frame kept on disk: width and height (u32, little-endian), then RGBA.
fn read(path: &PathBuf) -> Option<Frame> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut head = [0u8; 8];
    f.read_exact(&mut head).ok()?;
    let w = u32::from_le_bytes(head[..4].try_into().ok()?);
    let h = u32::from_le_bytes(head[4..].try_into().ok()?);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    f.read_to_end(&mut rgba).ok()?;
    (rgba.len() == (w * h * 4) as usize).then(|| Frame {
        width: w,
        height: h,
        rgba: Arc::from(rgba),
    })
}

fn write(path: &PathBuf, frame: &Frame) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let part = path.with_extension("part");
    let mut f = std::fs::File::create(&part)?;
    f.write_all(&frame.width.to_le_bytes())?;
    f.write_all(&frame.height.to_le_bytes())?;
    f.write_all(&frame.rgba)?;
    drop(f);
    std::fs::rename(&part, path)
}

impl Shared {
    fn runner(&self) -> Result<Arc<dyn Interpolator>, String> {
        let model = ModelTask::FrameInterpolation.model();
        let mut slot = lock(&self.runner);
        if let Some((id, r)) = slot.as_ref()
            && *id == model.id
        {
            return Ok(r.clone());
        }
        let r = runner(model).ok_or("Optical flow is not in this build.")?;
        *slot = Some((model.id, r.clone()));
        Ok(r)
    }

    /// The frame under `key`, in memory or on disk.
    fn find(&self, key: u64, folder: &str) -> Option<Frame> {
        let mut made = lock(&self.made);
        if let Some(f) = made.frames.get(&key) {
            return Some(f.clone());
        }
        if made.missing.contains(&key) {
            return None;
        }
        match read(&file(folder, key)) {
            Some(f) => {
                Self::keep(&mut made, key, f.clone());
                Some(f)
            }
            None => {
                made.missing.insert(key);
                None
            }
        }
    }

    fn keep(made: &mut Made, key: u64, frame: Frame) {
        made.missing.remove(&key);
        if made.frames.insert(key, frame).is_none() {
            made.order.push_back(key);
        }
        while made.order.len() > KEPT {
            if let Some(old) = made.order.pop_front() {
                made.frames.remove(&old);
            }
        }
    }

    /// Whether `key` failed while the model it would run on was there.
    fn failed(&self, key: u64) -> bool {
        let downloaded = ModelTask::FrameInterpolation.model().is_downloaded();
        let mut failed = lock(&self.failed);
        match failed.get(&key) {
            // Downloaded since: try again.
            Some(false) if downloaded => {
                failed.remove(&key);
                false
            }
            Some(_) => true,
            None => false,
        }
    }

    fn make(&self, job: &Job) -> Option<Frame> {
        let made = self
            .runner()
            .and_then(|r| r.between(&job.a, &job.b, job.at).map_err(|e| e.to_string()));
        match made {
            Ok(frame) => {
                if let Err(e) = write(&file(&job.folder, job.key), &frame) {
                    eprintln!("koo-analysis: could not keep an optical flow frame: {e}");
                }
                Self::keep(&mut lock(&self.made), job.key, frame.clone());
                self.version.fetch_add(1, Ordering::SeqCst);
                Some(frame)
            }
            Err(e) => {
                let downloaded = ModelTask::FrameInterpolation.model().is_downloaded();
                if downloaded {
                    eprintln!("koo-analysis: optical flow failed: {e}");
                }
                lock(&self.failed).insert(job.key, downloaded);
                None
            }
        }
    }

    /// Make waiting frames, newest first, for good.
    fn work(&self) {
        loop {
            let job = {
                let mut waiting = lock(&self.waiting);
                loop {
                    if let Some(job) = waiting.pop_back() {
                        break job;
                    }
                    waiting = self.wake.wait(waiting).unwrap_or_else(|e| e.into_inner());
                }
            };
            if self.find(job.key, &job.folder).is_none() {
                self.make(&job);
            }
        }
    }
}

impl InterpolationService for LocalInterpolation {
    fn available(&self) -> Result<(), String> {
        if !Self::in_build() {
            return Err("Optical flow is not in this build.".into());
        }
        let model = ModelTask::FrameInterpolation.model();
        if !model.is_downloaded() {
            return Err(format!(
                "{} is not downloaded: Settings → Models.",
                model.name
            ));
        }
        Ok(())
    }

    fn between(&self, request: &Between<'_>, wait: bool) -> Option<Frame> {
        let (key, folder) = key(request);
        if let Some(frame) = self.shared.find(key, &folder) {
            return Some(frame);
        }
        if self.available().is_err() || self.shared.failed(key) {
            return None;
        }
        let job = Job {
            key,
            folder,
            a: request.a.1.clone(),
            b: request.b.1.clone(),
            at: request.at,
        };
        if wait {
            return self.shared.make(&job);
        }
        let mut waiting = lock(&self.shared.waiting);
        if !waiting.iter().any(|j| j.key == key) {
            waiting.push_back(job);
            while waiting.len() > WAITING {
                waiting.pop_front();
            }
            self.shared.wake.notify_one();
        }
        None
    }

    fn version(&self) -> u64 {
        self.shared.version.load(Ordering::SeqCst)
    }
}
