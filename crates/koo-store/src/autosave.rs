//! Debounced background autosave. The UI thread hands over a snapshot and
//! returns at once; a worker thread coalesces snapshots and writes the newest
//! one [`DEBOUNCE`] after the last change (at most [`MAX_DELAY`] after the
//! first unsaved one, so a long drag still gets saved).
//!
//! Debounced writes go to the autosave row. Closing cleanly (drop, or
//! [`Autosaver::close`]) promotes the latest state into the project row, so
//! an autosave row found on the next open means the editor crashed.

use crate::Store;
use koo_core::history::History;
use koo_core::ids::ProjectId;
use koo_core::model::Project;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub const DEBOUNCE: Duration = Duration::from_secs(1);
pub const MAX_DELAY: Duration = Duration::from_secs(5);

enum Msg {
    Snapshot(Box<(Project, History)>),
    /// Write anything pending now; reply when done.
    Flush(Sender<()>),
    /// Clean close of the current project; reply when done.
    Close(Sender<()>),
}

pub struct Autosaver {
    tx: Option<Sender<Msg>>,
    worker: Option<JoinHandle<()>>,
}

impl Autosaver {
    /// Start the worker. It takes its own connection to the store.
    pub fn spawn(store: Store) -> Self {
        let (tx, rx) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("koo-autosave".into())
            .spawn(move || run(store, rx))
            .ok();
        Self {
            tx: Some(tx),
            worker,
        }
    }

    /// Queue a snapshot. Cheap: no I/O or serialization on the caller's
    /// thread. A snapshot of another project first closes the previous one.
    pub fn save(&self, project: Project, history: History) {
        self.send(Msg::Snapshot(Box::new((project, history))));
    }

    /// Write pending changes now (as an autosave) and wait for it.
    pub fn flush(&self) {
        self.request(Msg::Flush);
    }

    /// Cleanly close the current project and wait: pending changes are
    /// written and promoted, leaving nothing to recover.
    pub fn close(&self) {
        self.request(Msg::Close);
    }

    fn send(&self, msg: Msg) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(msg);
        }
    }

    fn request(&self, make: fn(Sender<()>) -> Msg) {
        let (ack, done) = mpsc::channel();
        self.send(make(ack));
        let _ = done.recv();
    }
}

impl Drop for Autosaver {
    fn drop(&mut self) {
        self.close();
        self.tx = None;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run(store: Store, rx: Receiver<Msg>) {
    let mut pending: Option<Box<(Project, History)>> = None;
    // Project whose autosave row this session may have written.
    let mut current: Option<ProjectId> = None;
    let mut first_change = Instant::now();
    let mut last_change = Instant::now();

    let write = |pending: &mut Option<Box<(Project, History)>>| {
        if let Some(snapshot) = pending.take() {
            let (project, history) = &*snapshot;
            if let Err(e) = store.write_autosave(project, history) {
                eprintln!("koo-store: autosave failed: {e:#}");
            }
        }
    };
    let close = |pending: &mut Option<Box<(Project, History)>>, current: Option<ProjectId>| {
        let result = match pending.take() {
            Some(snapshot) => store.save_project(&snapshot.0, &snapshot.1),
            None => current
                .map_or(Ok(false), |id| store.promote_autosave(id))
                .map(|_| ()),
        };
        if let Err(e) = result {
            eprintln!("koo-store: save on close failed: {e:#}");
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
                let id = snapshot.0.id;
                if current.is_some_and(|c| c != id) {
                    close(&mut pending, current);
                }
                current = Some(id);
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
            Msg::Close(ack) => {
                close(&mut pending, current);
                current = None;
                let _ = ack.send(());
            }
        }
    }
    close(&mut pending, current);
}
