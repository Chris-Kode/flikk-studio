//! Queues for heavy background work. Jobs run in the order they were
//! queued, at most `limit` at a time, on threads of the queue's own that
//! end once nothing is left to run, and report their progress as
//! [`JobStatus`]. Work queued under a key that is already queued or
//! running is that same job. A job can be cancelled by its key: one not
//! started never runs, and one running sees [`JobProgress::cancelled`].

use crate::ids::{JobId, MediaId};
use crate::services::{JobState, JobStatus};
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

/// What a job does; `Err` says why it failed.
type Work = Box<dyn FnOnce(&JobProgress) -> Result<(), String> + Send>;

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
    /// The keys of the jobs queued or running, and their cancel flags.
    keys: Vec<(K, JobId, Arc<AtomicBool>)>,
    waiting: VecDeque<(JobId, Arc<AtomicBool>, Work)>,
    /// Threads running jobs.
    threads: usize,
}

/// Where a running job reports how far it got.
#[derive(Clone)]
pub struct JobProgress {
    jobs: Arc<Mutex<Vec<JobStatus>>>,
    id: JobId,
    cancel: Arc<AtomicBool>,
}

impl JobProgress {
    /// Whether the job was cancelled ([`JobQueue::cancel`]): it should
    /// stop as soon as it can; what it returns then is not listed.
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// 0..1 of the job done.
    pub fn set(&self, fraction: f32) {
        set_state(
            &self.jobs,
            self.id,
            JobState::Running {
                fraction: fraction.clamp(0.0, 1.0),
            },
        );
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
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

    /// Queue `work` as the job `label`, about `media` if any. When a job
    /// under `key` is queued or running already, that job's id instead.
    pub fn push(
        &self,
        key: K,
        label: impl Into<String>,
        media: Option<MediaId>,
        work: impl FnOnce(&JobProgress) -> Result<(), String> + Send + 'static,
    ) -> JobId {
        let shared = &self.shared;
        let mut queue = lock(&shared.queue);
        if let Some(&(_, id, _)) = queue.keys.iter().find(|(k, _, _)| *k == key) {
            return id;
        }
        let id = JobId::new();
        lock(&shared.jobs).push(JobStatus {
            id,
            label: label.into(),
            media,
            state: JobState::Queued,
        });
        let cancel = Arc::new(AtomicBool::new(false));
        queue.keys.push((key, id, cancel.clone()));
        queue.waiting.push_back((id, cancel, Box::new(work)));
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
        let mut queue = lock(&self.shared.queue);
        let Some(i) = queue.keys.iter().position(|(k, _, _)| k == key) else {
            return false;
        };
        let (_, id, cancel) = queue.keys.remove(i);
        cancel.store(true, Ordering::Relaxed);
        queue.waiting.retain(|(waiting, _, _)| *waiting != id);
        lock(&self.shared.jobs).retain(|j| j.id != id);
        true
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
            let Some((id, cancel, work)) = next else {
                return;
            };
            let progress = JobProgress {
                jobs: self.jobs.clone(),
                id,
                cancel,
            };
            progress.set(0.0);
            let result = catch_unwind(AssertUnwindSafe(|| work(&progress)))
                .unwrap_or_else(|_| Err(format!("{} stopped unexpectedly", self.name)));
            let mut queue = lock(&self.queue);
            queue.keys.retain(|(_, k, _)| *k != id);
            // A cancelled job left the list already.
            set_state(&self.jobs, id, outcome(result));
        }
    }
}
