//! One generation while it runs: where its provider says it is, and
//! whether the user stopped it.

use super::{CloudError, CloudResult};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Where a generation is, as its provider tells it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Stage {
    /// Sent, or made in one request that can't be followed or stopped.
    #[default]
    Starting,
    /// Waiting its turn; how many are ahead of it, when the provider says.
    Queued(Option<u32>),
    /// Being made.
    Running,
    /// The file coming down, 0..1.
    Downloading(f32),
    /// Followed, but the last looks at it failed (no connection, a failing
    /// server): where it is isn't known until one gets through. See
    /// [`GenerationTask::looked`].
    Reconnecting,
}

impl Stage {
    /// Whether it can still be stopped: a provider follows it, and it isn't
    /// made yet (what is made is paid for, and kept). See also
    /// [`GenerationTask::can_stop`], for a provider that can't stop one.
    pub fn can_stop(self) -> bool {
        matches!(self, Self::Queued(_) | Self::Running | Self::Reconnecting)
    }
}

/// Failed looks in a row after which a generation shows as
/// [`Stage::Reconnecting`]: one can be a blip.
const RECONNECTING_AFTER: u32 = 2;

/// One generation while it runs: where it is, and whether the user stopped
/// it. Clones share it: the thread making it reports, the UI reads and
/// stops.
#[derive(Clone, Debug, Default)]
pub struct GenerationTask(Arc<TaskState>);

#[derive(Debug, Default)]
struct TaskState {
    /// The last stage the provider told of.
    stage: Mutex<Stage>,
    /// Whether the last looks at it failed: shown as reconnecting over
    /// `stage`, which comes back once one gets through.
    reconnecting: AtomicBool,
    cancelled: AtomicBool,
    unstoppable: AtomicBool,
}

impl GenerationTask {
    /// Where it is, as shown: [`Stage::Reconnecting`] while the last
    /// looks at it failed.
    pub fn stage(&self) -> Stage {
        if self.0.reconnecting.load(Ordering::Relaxed) {
            Stage::Reconnecting
        } else {
            self.told()
        }
    }

    /// The last stage the provider told of.
    fn told(&self) -> Stage {
        *flikk_os::sync::lock(&self.0.stage)
    }

    pub fn report(&self, stage: Stage) {
        *flikk_os::sync::lock(&self.0.stage) = stage;
    }

    /// Said by a provider following it after each look at where it is:
    /// how many failed in a row, 0 when this one got through. From a
    /// couple on it shows as [`Stage::Reconnecting`]; the stage reported
    /// last shows again once a look gets through.
    pub fn looked(&self, failures: u32) {
        self.0
            .reconnecting
            .store(failures >= RECONNECTING_AFTER, Ordering::Relaxed);
    }

    /// Said by a provider that follows it but has no way to stop it: it
    /// runs to the end, and is paid for.
    pub fn unstoppable(&self) {
        self.0.unstoppable.store(true, Ordering::Relaxed);
    }

    /// Whether stopping it now would stop it.
    pub fn can_stop(&self) -> bool {
        // Out of reach for now, it can be stopped as it could before.
        !self.0.unstoppable.load(Ordering::Relaxed) && self.told().can_stop()
    }

    /// Ask the provider to stop it; what is already made is kept.
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Relaxed)
    }

    /// [`CloudError::Cancelled`] once stopped, for a provider between
    /// steps.
    pub fn check(&self) -> CloudResult<()> {
        if self.cancelled() {
            Err(CloudError::Cancelled)
        } else {
            Ok(())
        }
    }
}
