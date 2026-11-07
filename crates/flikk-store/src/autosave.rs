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

use crate::project_file::{autosave_path, remove_autosave, write_file};
use flikk_core::ids::ProjectId;
use flikk_core::model::Project;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub const DEBOUNCE: Duration = Duration::from_secs(1);
pub const MAX_DELAY: Duration = Duration::from_secs(5);

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

pub struct Autosaver {
    tx: Option<Sender<Msg>>,
    worker: Option<JoinHandle<()>>,
}

impl Autosaver {
    pub fn spawn() -> Self {
        let (tx, rx) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("flikk-autosave".into())
            .spawn(move || run(rx))
            .ok();
        Self {
            tx: Some(tx),
            worker,
        }
    }

    /// Queue a snapshot. Cheap: no I/O or serialization on the caller's
    /// thread.
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

    fn send(&self, msg: Msg) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(msg);
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

fn run(rx: Receiver<Msg>) {
    let mut pending: Option<Box<Snapshot>> = None;
    let mut first_change = Instant::now();
    let mut last_change = Instant::now();

    let write = |pending: &mut Option<Box<Snapshot>>| {
        if let Some(s) = pending.take() {
            let path = autosave_path(s.project.id);
            let relative_to = s.file.as_deref().unwrap_or(&path);
            let written = write_file(&path, relative_to, &s.project, s.file.as_deref());
            if let Err(e) = written {
                eprintln!("flikk-store: autosave failed: {e:#}");
            }
        }
    };

    loop {
        let msg = if pending.is_some() {
            let deadline = (last_change + DEBOUNCE).min(first_change + MAX_DELAY);
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(msg) => msg,
                Err(RecvTimeoutError::Timeout) => {
                    write(&mut pending);
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match rx.recv() {
                Ok(msg) => msg,
                Err(_) => break,
            }
        };
        match msg {
            Msg::Snapshot(snapshot) => {
                // Another project's changes are kept before switching.
                if pending
                    .as_ref()
                    .is_some_and(|p| p.project.id != snapshot.project.id)
                {
                    write(&mut pending);
                }
                if pending.is_none() {
                    first_change = Instant::now();
                }
                last_change = Instant::now();
                pending = Some(snapshot);
            }
            Msg::Flush(ack) => {
                write(&mut pending);
                let _ = ack.send(());
            }
            Msg::Clear(id, ack) => {
                if pending.as_ref().is_some_and(|p| p.project.id == id) {
                    pending = None;
                }
                remove_autosave(id);
                let _ = ack.send(());
            }
        }
    }
    write(&mut pending);
}
