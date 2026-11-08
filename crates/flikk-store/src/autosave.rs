//! Debounced background autosave for crash recovery. The UI thread hands
//! over a snapshot of unsaved changes and returns at once; a worker thread
//! coalesces snapshots and writes the newest one [`DEBOUNCE`] after the last
//! change (at most [`MAX_DELAY`] after the first unsaved one, so a long drag
//! still gets saved).
//!
//! Snapshots go to the project's autosave file
//! ([`crate::project_file::autosave_path`]), never to the user's `.flikk`
//! file. Saving or discarding the changes [`Autosaver::clear`]s it, so an
//! autosave found on the next start means the editor did not close cleanly.
//!
//! A snapshot whose write fails is kept and tried again, [`RETRY`] later at
//! first and backing off to [`MAX_RETRY`]; a newer one of the same project
//! takes its place. [`Autosaver::failing`] says why while it fails.

use crate::project_file::{autosave_path, remove_autosave, write_file};
use flikk_core::ids::ProjectId;
use flikk_core::model::Project;
use flikk_os::sync::lock;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub const DEBOUNCE: Duration = Duration::from_secs(1);
pub const MAX_DELAY: Duration = Duration::from_secs(5);
/// How long after a failed write it is first tried again.
const RETRY: Duration = Duration::from_secs(5);
/// The longest wait between tries, doubling from [`RETRY`].
const MAX_RETRY: Duration = Duration::from_secs(60);

/// Unsaved state of the open project.
pub struct Snapshot {
    pub project: Project,
    /// The file it belongs to; `None` while never saved.
    pub file: Option<PathBuf>,
}

enum Msg {
    Snapshot(Box<Snapshot>),
    /// Write anything pending now; reply when done.
    Flush(Sender<()>),
    /// Drop the project's pending snapshot and its autosave; reply when done.
    Clear(ProjectId, Sender<()>),
}

/// Why autosaving fails, while it does.
#[derive(Default)]
struct Failure {
    message: Option<String>,
    /// [`Autosaver::take_failure`] gave it out.
    reported: bool,
}

pub struct Autosaver {
    tx: Option<Sender<Msg>>,
    worker: Option<JoinHandle<()>>,
    /// Writes on the caller's thread when the worker could not be started.
    inline: Option<Mutex<Worker>>,
    failure: Arc<Mutex<Failure>>,
}

impl Autosaver {
    pub fn spawn() -> Self {
        let failure = Arc::new(Mutex::new(Failure::default()));
        let (tx, rx) = mpsc::channel();
        let shared = failure.clone();
        let spawned = std::thread::Builder::new()
            .name("flikk-autosave".into())
            .spawn(move || run(rx, Worker::new(shared)));
        match spawned {
            Ok(worker) => Self {
                tx: Some(tx),
                worker: Some(worker),
                inline: None,
                failure,
            },
            Err(e) => {
                eprintln!("flikk-store: autosave thread failed to start, saving inline: {e}");
                Self {
                    tx: None,
                    worker: None,
                    inline: Some(Mutex::new(Worker::new(failure.clone()))),
                    failure,
                }
            }
        }
    }

    /// Queue a snapshot. Cheap: no I/O or serialization on the caller's
    /// thread (unless the worker could not be started; then it is written
    /// here, at once).
    pub fn save(&self, snapshot: Snapshot) {
        self.send(Msg::Snapshot(Box::new(snapshot)));
    }

    /// Write pending changes now and wait for it.
    pub fn flush(&self) {
        self.request(Msg::Flush);
    }

    /// The project's changes were saved or discarded: forget them and wait.
    pub fn clear(&self, project: ProjectId) {
        self.request(|ack| Msg::Clear(project, ack));
    }

    /// Why the last write failed, while unsaved changes are not protected;
    /// `None` once a write succeeds or the changes are cleared.
    pub fn failing(&self) -> Option<String> {
        lock(&self.failure).message.clone()
    }

    /// [`Self::failing`], once per run of failures: to tell the user
    /// without repeating it on every change.
    pub fn take_failure(&self) -> Option<String> {
        let mut failure = lock(&self.failure);
        if failure.reported {
            return None;
        }
        failure.reported = true;
        failure.message.clone()
    }

    fn send(&self, msg: Msg) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(msg);
        } else if let Some(inline) = &self.inline {
            let mut worker = lock(inline);
            let snapshot = matches!(msg, Msg::Snapshot(_));
            worker.handle(msg);
            // No thread to wait out the debounce: write now, but still back
            // off from failures.
            if snapshot && worker.retry_at.is_none_or(|at| at <= Instant::now()) {
                worker.write(|_| true);
            }
        }
    }

    fn request(&self, make: impl FnOnce(Sender<()>) -> Msg) {
        let (ack, done) = mpsc::channel();
        self.send(make(ack));
        let _ = done.recv();
    }
}

impl Drop for Autosaver {
    /// Pending changes are written, so they are recovered next time.
    fn drop(&mut self) {
        self.flush();
        self.tx = None;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// The snapshots waiting to be written, and when.
struct Worker {
    /// At most one per project: the one being edited, and others' whose
    /// write failed.
    pending: Vec<Snapshot>,
    first_change: Instant,
    last_change: Instant,
    /// After a failed write: not before then.
    retry_at: Option<Instant>,
    /// The wait before the next retry.
    backoff: Duration,
    failure: Arc<Mutex<Failure>>,
}

impl Worker {
    fn new(failure: Arc<Mutex<Failure>>) -> Self {
        Self {
            pending: Vec::new(),
            first_change: Instant::now(),
            last_change: Instant::now(),
            retry_at: None,
            backoff: RETRY,
            failure,
        }
    }

    /// When to write what is pending, if anything is.
    fn deadline(&self) -> Option<Instant> {
        if self.pending.is_empty() {
            return None;
        }
        let due = (self.last_change + DEBOUNCE).min(self.first_change + MAX_DELAY);
        Some(self.retry_at.map_or(due, |at| due.max(at)))
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Snapshot(snapshot) => {
                let id = snapshot.project.id;
                // Another project's changes are kept before switching; while
                // backing off they stay pending, for the next try.
                if self.retry_at.is_none_or(|at| at <= Instant::now()) {
                    self.write(|p| p.project.id != id);
                }
                if self.pending.is_empty() {
                    self.first_change = Instant::now();
                }
                self.last_change = Instant::now();
                // A newer snapshot takes the place of one that failed.
                self.pending.retain(|p| p.project.id != id);
                self.pending.push(*snapshot);
            }
            Msg::Flush(ack) => {
                self.write(|_| true);
                let _ = ack.send(());
            }
            Msg::Clear(id, ack) => {
                self.pending.retain(|p| p.project.id != id);
                if self.pending.is_empty() {
                    self.succeeded();
                }
                remove_autosave(id);
                let _ = ack.send(());
            }
        }
    }

    /// Write the pending snapshots `which` picks; those that fail stay.
    fn write(&mut self, which: impl Fn(&Snapshot) -> bool) {
        let mut error = None;
        self.pending.retain(|s| {
            if !which(s) {
                return true;
            }
            let path = autosave_path(s.project.id);
            let relative_to = s.file.as_deref().unwrap_or(&path);
            match write_file(&path, relative_to, &s.project, s.file.as_deref()) {
                Ok(()) => false,
                Err(e) => {
                    error = Some(format!("{e:#}"));
                    true
                }
            }
        });
        match error {
            Some(message) => self.failed(message),
            None if self.pending.is_empty() => self.succeeded(),
            None => {}
        }
    }

    fn failed(&mut self, message: String) {
        eprintln!("flikk-store: autosave failed: {message}");
        self.retry_at = Some(Instant::now() + self.backoff);
        self.backoff = (self.backoff * 2).min(MAX_RETRY);
        let mut failure = lock(&self.failure);
        if failure.message.is_none() {
            failure.reported = false;
        }
        failure.message = Some(message);
    }

    /// Nothing is left that failed to write.
    fn succeeded(&mut self) {
        self.retry_at = None;
        self.backoff = RETRY;
        *lock(&self.failure) = Failure::default();
    }
}

fn run(rx: Receiver<Msg>, mut worker: Worker) {
    loop {
        let msg = match worker.deadline() {
            Some(deadline) => {
                match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                    Ok(msg) => msg,
                    Err(RecvTimeoutError::Timeout) => {
                        worker.write(|_| true);
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
            None => match rx.recv() {
                Ok(msg) => msg,
                Err(_) => break,
            },
        };
        worker.handle(msg);
    }
    worker.write(|_| true);
}
