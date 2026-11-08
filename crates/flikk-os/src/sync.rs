//! Locks that keep working after a panic, and the text of a caught panic.

use std::any::Any;
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::Duration;

/// `mutex` locked, even after a panic while it was held. The data behind
/// Flikk's locks (caches, queues, flags) stays sound across a panic, so
/// one panic must not break every later call.
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Wait on `condvar` with `guard`, poison-tolerant like [`lock`].
pub fn wait<'a, T>(condvar: &Condvar, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
    condvar.wait(guard).unwrap_or_else(|e| e.into_inner())
}

/// Wait on `condvar` with `guard` for at most `timeout`, poison-tolerant
/// like [`lock`]. Whether it timed out is not told: callers check their
/// condition again either way.
pub fn wait_timeout<'a, T>(
    condvar: &Condvar,
    guard: MutexGuard<'a, T>,
    timeout: Duration,
) -> MutexGuard<'a, T> {
    match condvar.wait_timeout(guard, timeout) {
        Ok((guard, _)) => guard,
        Err(e) => e.into_inner().0,
    }
}

/// What a panic said, when it said it with a string (`panic!("…")`).
pub fn panic_detail(payload: &(dyn Any + Send)) -> Option<&str> {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
}

/// "`what` stopped unexpectedly", with what the panic said after a colon
/// when it said something: the failure a caught panic is reported as.
pub fn panic_message(what: &str, payload: &(dyn Any + Send)) -> String {
    match panic_detail(payload) {
        Some(detail) => format!("{what} stopped unexpectedly: {detail}"),
        None => format!("{what} stopped unexpectedly"),
    }
}
