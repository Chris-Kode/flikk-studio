//! Queues for heavy background work. Jobs run in the order they were
//! queued, at most `limit` at a time, on threads of the queue's own that
//! end once nothing is left to run, and report their progress as
//! [`JobStatus`]. A job someone asked for and waits on goes ahead of those
//! queued in the background ([`JobQueue::push_first`]); each group keeps
//! the order queued. Work queued under a key that is already queued or
//! running is that same job. A job can be cancelled by its key: one not
//! started never runs, and one running sees [`JobProgress::cancelled`].

use crate::ids::{JobId, MediaId};
use crate::services::{JobState, JobStatus};
use flikk_os::sync::lock;
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// What a job does; `Err` says why it failed.
type Work = Box<dyn FnOnce(&JobProgress) -> Result<(), String> + Send>;

/// Whether someone waits on a new job, which decides where it waits.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Lane {
    /// Behind every job queued.
    Background,
    /// Behind the jobs asked for, ahead of those in the background.
    Asked,
}

/// The jobs a cancel looks at.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Among {
    All,
    WaitingInBackground,
}

/// What a job queued or running shares with its [`JobProgress`].
#[derive(Clone, Default)]
struct Flags {
    cancel: Arc<AtomicBool>,
    /// Someone asked for it: from the start, or once it was waiting in
    /// the background.
    asked: Arc<AtomicBool>,
}

impl Flags {
    fn asked(&self) -> bool {
        self.asked.load(Ordering::Relaxed)
    }
}

/// Cheap to clone; clones share one queue.
pub struct JobQueue<K> {
    shared: Arc<Shared<K>>,
}

impl<K> Clone for JobQueue<K> {
    fn clone(&self) -> Self {
        Self {
            shared: self.shared.clone(),
        }
    }
}

struct Shared<K> {
    /// Names the queue's threads.
    name: String,
    limit: usize,
    /// Locked before `jobs` when both are.
    queue: Mutex<Queue<K>>,
    jobs: Arc<Mutex<Vec<JobStatus>>>,
}

struct Queue<K> {
    /// The keys of the jobs queued or running, and their flags.
    keys: Vec<(K, JobId, Flags)>,
    /// Those asked for first, then those in the background.
    waiting: VecDeque<(JobId, Flags, Work)>,
    /// Threads running jobs.
    threads: usize,
}

/// Where a running job reports how far it got.
#[derive(Clone)]
pub struct JobProgress {
    jobs: Arc<Mutex<Vec<JobStatus>>>,
    id: JobId,
    flags: Flags,
}

impl JobProgress {
    /// Whether the job was cancelled ([`JobQueue::cancel`]): it should
    /// stop as soon as it can; what it returns then is not listed.
    pub fn cancelled(&self) -> bool {
        self.flags.cancel.load(Ordering::Relaxed)
    }

    /// Whether someone asked for the job ([`JobQueue::push_first`]), when
    /// it was queued or while it waited; `false` for one only queued in
    /// the background ([`JobQueue::push`]).
    pub fn asked(&self) -> bool {
        self.flags.asked()
    }

    /// 0..1 of the job done. Ignored once the job has finished or failed,
    /// so a late report from a clone does not make it run again.
    pub fn set(&self, fraction: f32) {
        if let Some(job) = lock(&self.jobs).iter_mut().find(|j| j.id == self.id)
            && matches!(job.state, JobState::Queued | JobState::Running { .. })
        {
            job.state = JobState::Running {
                fraction: fraction.clamp(0.0, 1.0),
            };
        }
    }
}

fn set_state(jobs: &Mutex<Vec<JobStatus>>, id: JobId, state: JobState) {
    if let Some(job) = lock(jobs).iter_mut().find(|j| j.id == id) {
        job.state = state;
    }
}

fn outcome(result: Result<(), String>) -> JobState {
    match result {
        Ok(()) => JobState::Done,
        Err(message) => JobState::Failed { message },
    }
}

impl<K: PartialEq + Send + 'static> JobQueue<K> {
    /// Runs at most `limit` jobs at once (at least one), on threads named
    /// `name`.
    pub fn new(name: impl Into<String>, limit: usize) -> Self {
        Self {
            shared: Arc::new(Shared {
                name: name.into(),
                limit: limit.max(1),
                queue: Mutex::new(Queue {
                    keys: Vec::new(),
                    waiting: VecDeque::new(),
                    threads: 0,
                }),
                jobs: Arc::default(),
            }),
        }
    }

    /// Queue `work` as the job `label`, about `media` if any, in the
    /// background: behind every job queued. When a job under `key` is
    /// queued or running already, that job's id instead.
    pub fn push(
        &self,
        key: K,
        label: impl Into<String>,
        media: Option<MediaId>,
        work: impl FnOnce(&JobProgress) -> Result<(), String> + Send + 'static,
    ) -> JobId {
        self.enqueue(key, label.into(), media, Box::new(work), Lane::Background)
    }

    /// [`Self::push`] for a job someone asked for and waits on: behind the
    /// others asked for, ahead of those in the background. A job under
    /// `key` waiting in the background moves there and counts as asked
    /// for; one running stays as it is.
    pub fn push_first(
        &self,
        key: K,
        label: impl Into<String>,
        media: Option<MediaId>,
        work: impl FnOnce(&JobProgress) -> Result<(), String> + Send + 'static,
    ) -> JobId {
        self.enqueue(key, label.into(), media, Box::new(work), Lane::Asked)
    }

    fn enqueue(
        &self,
        key: K,
        label: String,
        media: Option<MediaId>,
        work: Work,
        lane: Lane,
    ) -> JobId {
        let shared = &self.shared;
        let mut queue = lock(&shared.queue);
        if let Some((_, id, flags)) = queue.keys.iter().find(|(k, _, _)| *k == key) {
            let (id, flags) = (*id, flags.clone());
            if lane == Lane::Asked && !flags.asked() {
                flags.asked.store(true, Ordering::Relaxed);
                if let Some(at) = queue.waiting.iter().position(|(w, _, _)| *w == id)
                    && let Some(job) = queue.waiting.remove(at)
                {
                    let at = queue.asked_waiting();
                    queue.waiting.insert(at, job);
                }
            }
            return id;
        }
        let id = JobId::new();
        lock(&shared.jobs).push(JobStatus {
            id,
            label,
            media,
            state: JobState::Queued,
        });
        let flags = Flags::default();
        queue.keys.push((key, id, flags.clone()));
        match lane {
            Lane::Background => queue.waiting.push_back((id, flags, work)),
            Lane::Asked => {
                flags.asked.store(true, Ordering::Relaxed);
                let at = queue.asked_waiting();
                queue.waiting.insert(at, (id, flags, work));
            }
        }
        if queue.threads < shared.limit {
            queue.threads += 1;
            drop(queue);
            self.spawn();
        }
        id
    }

    /// List a job that needed no work as finished with `result`.
    pub fn finished(
        &self,
        label: impl Into<String>,
        media: Option<MediaId>,
        result: Result<(), String>,
    ) -> JobId {
        let id = JobId::new();
        lock(&self.shared.jobs).push(JobStatus {
            id,
            label: label.into(),
            media,
            state: outcome(result),
        });
        id
    }

    /// Every job not cleared, in the order queued.
    pub fn jobs(&self) -> Vec<JobStatus> {
        lock(&self.shared.jobs).clone()
    }

    /// Why job `id` failed, if it has.
    pub fn failed(&self, id: JobId) -> Option<String> {
        lock(&self.shared.jobs)
            .iter()
            .find(|j| j.id == id)
            .and_then(|j| match &j.state {
                JobState::Failed { message } => Some(message.clone()),
                _ => None,
            })
    }

    /// Cancel the job under `key`, queued or running: it leaves
    /// [`Self::jobs`] now, one queued never runs, and work pushed under
    /// `key` after is a new job. Whether there was one.
    pub fn cancel(&self, key: &K) -> bool {
        self.cancel_where(|k| k == key) > 0
    }

    /// [`Self::cancel`] every job whose key `cancels` picks. How many there
    /// were.
    pub fn cancel_where(&self, cancels: impl FnMut(&K) -> bool) -> usize {
        self.cancel_picked(cancels, Among::All).len()
    }

    /// [`Self::cancel`] every job waiting in the background, never asked
    /// for, whose key `cancels` picks. Jobs running go on. Their keys, in
    /// the order they were queued.
    pub fn cancel_background_where(&self, cancels: impl FnMut(&K) -> bool) -> Vec<K> {
        self.cancel_picked(cancels, Among::WaitingInBackground)
    }

    /// Cancel the jobs `among` covers whose key `cancels` picks. Their
    /// keys.
    fn cancel_picked(&self, mut cancels: impl FnMut(&K) -> bool, among: Among) -> Vec<K> {
        let mut queue = lock(&self.shared.queue);
        let background: Vec<JobId> = match among {
            Among::All => Vec::new(),
            Among::WaitingInBackground => queue
                .waiting
                .iter()
                .filter(|(_, flags, _)| !flags.asked())
                .map(|(id, _, _)| *id)
                .collect(),
        };
        let picked: Vec<(K, JobId, Flags)> = queue
            .keys
            .extract_if(.., |(k, id, _)| {
                (among == Among::All || background.contains(id)) && cancels(k)
            })
            .collect();
        let ids: Vec<JobId> = picked.iter().map(|(_, id, _)| *id).collect();
        if !ids.is_empty() {
            queue
                .waiting
                .retain(|(waiting, _, _)| !ids.contains(waiting));
            lock(&self.shared.jobs).retain(|j| !ids.contains(&j.id));
        }
        picked
            .into_iter()
            .map(|(key, _, flags)| {
                flags.cancel.store(true, Ordering::Relaxed);
                key
            })
            .collect()
    }

    /// Drop finished and failed jobs from [`Self::jobs`].
    pub fn clear_finished(&self) {
        lock(&self.shared.jobs)
            .retain(|j| matches!(j.state, JobState::Queued | JobState::Running { .. }));
    }

    fn spawn(&self) {
        let shared = self.shared.clone();
        let spawned = std::thread::Builder::new()
            .name(self.shared.name.clone())
            .spawn(move || shared.work());
        if let Err(e) = spawned {
            let shared = &self.shared;
            let mut queue = lock(&shared.queue);
            queue.threads -= 1;
            if queue.threads == 0 {
                // Nothing else would ever run them.
                let message = format!("could not start {}: {e}", shared.name);
                for (id, _, _) in std::mem::take(&mut queue.waiting) {
                    queue.keys.retain(|(_, k, _)| *k != id);
                    set_state(&shared.jobs, id, outcome(Err(message.clone())));
                }
            }
        }
    }
}

impl<K> Queue<K> {
    /// How many jobs asked for wait: where the next one goes.
    fn asked_waiting(&self) -> usize {
        self.waiting
            .iter()
            .take_while(|(_, flags, _)| flags.asked())
            .count()
    }
}

impl<K: PartialEq> Shared<K> {
    /// Run waiting jobs until there are none. A job that panics fails, and
    /// the next one runs.
    fn work(self: Arc<Self>) {
        loop {
            let next = {
                let mut queue = lock(&self.queue);
                let next = queue.waiting.pop_front();
                if next.is_none() {
                    queue.threads -= 1;
                }
                next
            };
            let Some((id, flags, work)) = next else {
                return;
            };
            let progress = JobProgress {
                jobs: self.jobs.clone(),
                id,
                flags,
            };
            progress.set(0.0);
            let result =
                catch_unwind(AssertUnwindSafe(|| work(&progress))).unwrap_or_else(|panic| {
                    // The user sees the short message; the cause goes to the log.
                    let message = format!("{} stopped unexpectedly", self.name);
                    eprintln!("{}", flikk_os::sync::panic_message(&self.name, &*panic));
                    Err(message)
                });
            let mut queue = lock(&self.queue);
            queue.keys.retain(|(_, k, _)| *k != id);
            // A cancelled job left the list already.
            set_state(&self.jobs, id, outcome(result));
        }
    }
}
