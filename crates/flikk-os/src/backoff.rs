//! Failures remembered for a while, then tried again: a failure is never
//! cached for the session (AGENTS.md), and a retry does not spin.

use std::collections::HashMap;
use std::hash::Hash;
use std::time::{Duration, Instant};

/// How long a failure is left before the work is asked for again.
pub const RETRY_AFTER: Duration = Duration::from_secs(30);

/// Why each key last failed, and when. A failure older than
/// [`RETRY_AFTER`] no longer counts: the key is tried again.
#[derive(Debug)]
pub struct Failures<K> {
    failed: HashMap<K, (String, Instant)>,
}

impl<K> Default for Failures<K> {
    fn default() -> Self {
        Self {
            failed: HashMap::new(),
        }
    }
}

impl<K: Hash + Eq> Failures<K> {
    /// Remember that `key` failed now, with `message`.
    pub fn record(&mut self, key: K, message: impl Into<String>) {
        self.failed.insert(key, (message.into(), Instant::now()));
    }

    /// Why `key` failed, when it failed less than [`RETRY_AFTER`] ago.
    pub fn recent(&self, key: &K) -> Option<&str> {
        self.failed
            .get(key)
            .filter(|(_, at)| at.elapsed() < RETRY_AFTER)
            .map(|(message, _)| message.as_str())
    }

    /// When `key` failed, however long ago; `None` when it has not.
    pub fn failed_at(&self, key: &K) -> Option<Instant> {
        self.failed.get(key).map(|(_, at)| *at)
    }

    /// Forget `key`'s failure, e.g. once it succeeded. Whether there was one.
    pub fn clear(&mut self, key: &K) -> bool {
        self.failed.remove(key).is_some()
    }

    /// Forget every failure.
    pub fn clear_all(&mut self) {
        self.failed.clear();
    }
}
