//! What the analysis engines that make things on threads of their own
//! (depth maps, optical flow, camera paths, subject and person masks)
//! share.

use flikk_os::backoff::Failures;
use flikk_os::sync::{lock, panic_detail, wait, wait_timeout};
use std::any::Any;
use std::collections::HashSet;
use std::hash::Hash;
use std::sync::{Condvar, Mutex};
use std::time::Duration;

/// "`what` stopped unexpectedly: …" with what the panic said, or
/// "`what` stopped unexpectedly." when it said nothing: a caught panic as
/// the masks report it, a sentence the user reads.
pub(crate) fn stopped_sentence(what: &str, payload: &(dyn Any + Send)) -> String {
    match panic_detail(payload) {
        Some(said) => format!("{what} stopped unexpectedly: {said}"),
        None => format!("{what} stopped unexpectedly."),
    }
}

/// Failures of work that runs on a model, each left for
/// [`flikk_os::backoff::RETRY_AFTER`] before it is tried again, unless the
/// model was missing when it failed and is there now: downloading it may
/// be what fixes it.
pub(crate) struct ModelFailures<K> {
    /// Those that failed with the model there.
    with_model: Failures<K>,
    /// Those that failed without it.
    without_model: Failures<K>,
}

impl<K> Default for ModelFailures<K> {
    fn default() -> Self {
        Self {
            with_model: Failures::default(),
            without_model: Failures::default(),
        }
    }
}

impl<K: Hash + Eq> ModelFailures<K> {
    /// Remember that `key` failed now, with `message`; `model_here` says
    /// whether the model was there. Replaces an earlier failure of `key`.
    pub(crate) fn record(&mut self, key: K, message: impl Into<String>, model_here: bool) {
        let (kept, other) = if model_here {
            (&mut self.with_model, &mut self.without_model)
        } else {
            (&mut self.without_model, &mut self.with_model)
        };
        other.clear(&key);
        kept.record(key, message);
    }

    /// Why `key` failed, unless it is to be tried again: the time passed,
    /// or it failed without the model and `model_here` now.
    pub(crate) fn recent(&mut self, key: &K, model_here: bool) -> Option<&str> {
        if model_here {
            self.without_model.clear(key);
        }
        self.with_model
            .recent(key)
            .or_else(|| self.without_model.recent(key))
    }

    /// Forget `key`'s failure: it succeeded, or is tried again now anyway.
    pub(crate) fn clear(&mut self, key: &K) {
        self.with_model.clear(key);
        self.without_model.clear(key);
    }
}

/// Keys being worked on, each by one thread at a time, and their release
/// announced to the threads waiting for them: so a frame or an item is
/// made once even when the background and a render both want it.
pub(crate) struct Claims<K> {
    held: Mutex<HashSet<K>>,
    released: Condvar,
}

impl<K> Default for Claims<K> {
    fn default() -> Self {
        Self {
            held: Mutex::default(),
            released: Condvar::new(),
        }
    }
}

impl<K: Hash + Eq + Clone> Claims<K> {
    /// Claim `key` for this thread, or return what stops it. Before each
    /// try `done` is asked, the claims locked so no release slips past:
    /// its answer, when it gives one, is returned and nothing is claimed.
    /// While another thread holds `key`, `busy` is asked the same way;
    /// without an answer this waits for a release, or at most `step` when
    /// given (to ask `busy` again), and tries again.
    pub(crate) fn claim<T>(
        &self,
        key: &K,
        mut done: impl FnMut() -> Option<T>,
        mut busy: impl FnMut() -> Option<T>,
        step: Option<Duration>,
    ) -> Result<Claim<'_, K>, T> {
        let mut held = lock(&self.held);
        loop {
            if let Some(answer) = done() {
                return Err(answer);
            }
            if held.insert(key.clone()) {
                return Ok(Claim {
                    claims: self,
                    key: key.clone(),
                });
            }
            if let Some(answer) = busy() {
                return Err(answer);
            }
            held = match step {
                Some(step) => wait_timeout(&self.released, held, step),
                None => wait(&self.released, held),
            };
        }
    }

    /// Whether a thread holds `key` now.
    pub(crate) fn held(&self, key: &K) -> bool {
        lock(&self.held).contains(key)
    }

    /// Wake every thread waiting for a release, to ask its `busy` again
    /// (what it waits for was cancelled, say).
    pub(crate) fn wake_all(&self) {
        self.released.notify_all();
    }
}

/// A key claimed in [`Claims`], released when dropped, a panic included,
/// so the threads waiting for it go on.
pub(crate) struct Claim<'a, K: Hash + Eq> {
    claims: &'a Claims<K>,
    key: K,
}

impl<K: Hash + Eq> Drop for Claim<'_, K> {
    fn drop(&mut self) {
        lock(&self.claims.held).remove(&self.key);
        self.claims.released.notify_all();
    }
}
