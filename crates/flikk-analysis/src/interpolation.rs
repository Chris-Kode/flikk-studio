//! Optical flow slow motion: [`LocalInterpolation`] implements
//! `InterpolationService` on the frame interpolation model (RIFE,
//! `flikk-models`).
//!
//! A frame between two source frames is kept by the media's fingerprint
//! (its path without one), the two frames' source times, the moment
//! between them, its size and the model: in memory (the last [`KEPT`])
//! and, at preview sizes, under `paths::cache()/interpolated/` as QOI
//! ([`DiskCache`], the least recently used going past [`DISK_LIMIT`]), so
//! a frame is made once. Larger frames, a render's, stay in memory: a
//! render asks for each only while it is on it. The viewer asks without
//! waiting: frames are made on
//! one thread of their own, the newest asked first (the playhead moved
//! on), and blended until they are made. Renders wait for each. A frame
//! that fails is not tried again until the model is downloaded anew.

use flikk_core::disk_cache::{DiskCache, GB, Key};
use flikk_core::paths;
use flikk_core::services::{Between, Frame, InterpolationService};
use flikk_core::subject::Fnv;
use flikk_models::Task as ModelTask;
use flikk_models::interpolation::Interpolator;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};

/// Frames kept in memory.
const KEPT: usize = 48;
/// Frames kept on disk have at most this short side: the viewer's and the
/// render cache's.
const DISK_SIDE: u32 = 720;
/// Disk the kept frames may use.
const DISK_LIMIT: u64 = 4 * GB;
const MAGIC: [u8; 4] = *b"FLKI";
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
    failed: Mutex<HashMap<Key, bool>>,
    version: AtomicU64,
}

#[derive(Default)]
struct Made {
    frames: HashMap<Key, Frame>,
    order: VecDeque<Key>,
}

struct Job {
    key: Key,
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
            .name("flikk-interpolate".into())
            .spawn(move || worker.work());
        Self { shared }
    }

    /// Whether this build makes optical flow frames (the `interpolation`
    /// feature).
    pub fn in_build() -> bool {
        cfg!(feature = "interpolation")
    }
}

fn runner(model: &'static flikk_models::Model) -> Option<Arc<dyn Interpolator>> {
    #[cfg(feature = "interpolation")]
    {
        Some(Arc::from(flikk_models::interpolation::runner(model)))
    }
    #[cfg(not(feature = "interpolation"))]
    {
        let _ = model;
        None
    }
}

/// What a request asks of the picked model, as a name.
fn key(request: &Between<'_>) -> Key {
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
    // HDR frames look as their tone map makes them (`flikk_core::hdr`).
    if let Some(mapping) = flikk_core::hdr::Mapping::of(request.item) {
        h.write(mapping.key().as_bytes());
    }
    let mut source = Fnv::default();
    source.write(folder.as_bytes());
    let mut key = [0; 16];
    key[..8].copy_from_slice(&h.0.to_le_bytes());
    key[8..].copy_from_slice(&source.0.to_le_bytes());
    key
}

/// Optical flow frames on disk, read on first use.
fn disk() -> &'static DiskCache {
    static DISK: OnceLock<DiskCache> = OnceLock::new();
    DISK.get_or_init(|| DiskCache::open(&dir(), "v2", MAGIC, "qoi", DISK_LIMIT))
}

fn dir() -> PathBuf {
    paths::cache().join("interpolated")
}

/// Remove the optical flow frames kept on disk.
pub fn clear_disk() {
    disk().clear();
}

fn on_disk(frame: &Frame) -> bool {
    frame.width.min(frame.height) <= DISK_SIDE
}

fn read(key: &Key) -> Option<Frame> {
    let bytes = disk().read(key)?;
    let frame = qoi::decode_to_vec(&bytes)
        .ok()
        .filter(|(header, _)| header.channels == qoi::Channels::Rgba)
        .map(|(header, rgba)| Frame {
            width: header.width,
            height: header.height,
            rgba: Arc::from(rgba),
        });
    if frame.is_none() {
        disk().forget(key);
    }
    frame
}

fn write(key: &Key, frame: &Frame) {
    if !on_disk(frame) {
        return;
    }
    match qoi::encode_to_vec(&*frame.rgba, frame.width, frame.height) {
        Ok(bytes) => disk().write(key, &bytes),
        Err(e) => eprintln!("flikk-analysis: could not keep an optical flow frame: {e}"),
    }
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
    fn find(&self, key: Key) -> Option<Frame> {
        if let Some(f) = lock(&self.made).frames.get(&key) {
            return Some(f.clone());
        }
        let frame = read(&key)?;
        Self::keep(&mut lock(&self.made), key, frame.clone());
        Some(frame)
    }

    fn keep(made: &mut Made, key: Key, frame: Frame) {
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
    fn failed(&self, key: Key) -> bool {
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
                write(&job.key, &frame);
                Self::keep(&mut lock(&self.made), job.key, frame.clone());
                self.version.fetch_add(1, Ordering::SeqCst);
                Some(frame)
            }
            Err(e) => {
                let downloaded = ModelTask::FrameInterpolation.model().is_downloaded();
                if downloaded {
                    eprintln!("flikk-analysis: optical flow failed: {e}");
                }
                lock(&self.failed).insert(job.key, downloaded);
                None
            }
        }
    }

    /// Make waiting frames, newest first, for good.
    fn work(&self) {
        // Read the disk's index here rather than on the first frame drawn.
        disk();
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
            if self.find(job.key).is_none() {
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
            return Err("Download it in Settings → Integrations.".into());
        }
        Ok(())
    }

    fn between(&self, request: &Between<'_>, wait: bool) -> Option<Frame> {
        let key = key(request);
        if let Some(frame) = self.shared.find(key) {
            return Some(frame);
        }
        if self.available().is_err() || self.shared.failed(key) {
            return None;
        }
        let job = Job {
            key,
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
