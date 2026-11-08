//! What the analysis engines that make things on threads of their own
//! (depth maps, optical flow, camera paths, subject and person masks)
//! share.

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
