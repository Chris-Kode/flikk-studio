//! fal's queue: a request is submitted, followed while it waits and runs,
//! and its result read once it has one. A request can be cancelled until
//! it is made; fal answers `ALREADY_COMPLETED` after that, and what was
//! made is kept. A cancel fal takes is a request: a running one may still
//! be made, so it is followed until it stops or is made.

use super::{NAME, auth, error, failure};
use crate::http;
use flikk_core::cloud::{CloudError, CloudResult, GenerationTask, Stage};
use serde_json::Value;
use std::time::{Duration, Instant};

const QUEUE: &str = "https://queue.fal.run";

/// Between looks at where a request is.
const POLL: Duration = Duration::from_secs(3);

/// Between looks at whether the user stopped it, while waiting to poll.
const TICK: Duration = Duration::from_millis(250);

/// The longest a request is followed, queue and all.
const PATIENCE: Duration = Duration::from_secs(2 * 60 * 60);

/// How long a new request may be unknown to its status URL.
const FRESH: Duration = Duration::from_secs(60);

/// How long a status this code doesn't know is waited out.
const STRANGE: Duration = Duration::from_secs(10 * 60);

/// After a cancel fal couldn't be told of, before it is told again.
const CANCEL_AGAIN: Duration = Duration::from_secs(15);

/// How long a request fal said it would stop may stay queued.
const STOPPING: Duration = Duration::from_secs(60);

/// How long the result of a made request is asked for.
const RESULT_PATIENCE: Duration = Duration::from_secs(10 * 60);

/// Submit `input` to `endpoint` and follow it until it is made; its result.
/// [`CloudError::Cancelled`] when `task` was cancelled in time. A look that
/// fails for a while (no connection, a failing server) doesn't end it: the
/// request goes on at fal, and is paid for.
pub fn run(key: &str, endpoint: &str, input: &Value, task: &GenerationTask) -> CloudResult<Value> {
    let auth = auth(key);
    let headers = [("authorization", auth.as_str())];
    task.check()?;
    // Not retried on a server error: it may have been taken, and charged.
    let reply = http::post_json(&format!("{QUEUE}/{endpoint}"), &headers, input, false)?;
    if !reply.ok() {
        return Err(error(&reply));
    }
    let submitted = reply.json()?;
    let id = submitted["request_id"]
        .as_str()
        .ok_or_else(|| CloudError::Failed(format!("{NAME} did not take the request")))?;
    // The URLs it hands back are the ones to use; built ones are a fallback.
    let url = |field: &str, path: &str| {
        submitted[field]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{QUEUE}/{endpoint}/requests/{id}{path}"))
    };
    let status_url = url("status_url", "/status");
    let response_url = url("response_url", "");
    let cancel_url = url("cancel_url", "/cancel");
    task.report(Stage::Queued(position(&submitted)));

    let started = Instant::now();
    // When a stop may be asked of fal; none once fal said it would try.
    let mut stop_from = Some(started);
    // When fal said it would try to stop it.
    let mut stopping: Option<Instant> = None;
    let mut failures = 0;
    let mut last_failure = String::new();
    let mut strange: Option<Instant> = None;
    loop {
        if wait(task, http::backoff(POLL, failures), stop_from) {
            match cancel(&cancel_url, &headers) {
                Cancel::Stopped => return Err(CloudError::Cancelled),
                Cancel::Made => break,
                Cancel::Asked => {
                    stopping = Some(Instant::now());
                    stop_from = None;
                }
                Cancel::Unsent => stop_from = Some(Instant::now() + CANCEL_AGAIN),
            }
        }
        if started.elapsed() > PATIENCE {
            cancel(&cancel_url, &headers);
            if stopping.is_some() {
                return Err(CloudError::Cancelled);
            }
            return Err(CloudError::Failed(format!(
                "{NAME} took over two hours, so it was stopped{}",
                lately(&last_failure, failures)
            )));
        }
        let status = match http::look(&status_url, &headers, started.elapsed() < FRESH) {
            http::Look::Answer(status) => status,
            http::Look::Again(why) => {
                failures += 1;
                last_failure = why;
                continue;
            }
            // Gone, after fal said it would stop it: stopped.
            http::Look::Refused(_) if stopping.is_some() => return Err(CloudError::Cancelled),
            http::Look::Refused(reply) => return Err(error(&reply)),
        };
        failures = 0;
        let said = status["status"].as_str();
        if !matches!(said, Some("IN_QUEUE" | "IN_PROGRESS" | "COMPLETED")) {
            // A status fal didn't document: its end, or one waited out.
            let message = status["error"].as_str();
            if stopping.is_some() && (message.is_some() || said.is_some_and(http::ended)) {
                return Err(CloudError::Cancelled);
            }
            if let Some(message) = message {
                let kind = status["error_type"].as_str().unwrap_or_default();
                return Err(failure(500, kind, message.to_owned()));
            }
            let said = said.unwrap_or("no status");
            if http::ended(said) {
                return Err(CloudError::Failed(format!(
                    "{NAME} ended the request as {said}"
                )));
            }
            let since = *strange.get_or_insert_with(Instant::now);
            if stopping.is_some() && since.elapsed() > STOPPING {
                return Err(CloudError::Cancelled);
            }
            if since.elapsed() > STRANGE {
                cancel(&cancel_url, &headers);
                return Err(CloudError::Failed(format!(
                    "{NAME} answered {said} for ten minutes, so it was stopped (request {id})"
                )));
            }
            continue;
        }
        strange = None;
        match said {
            Some("IN_QUEUE") => {
                // Still waiting its turn, though fal said it would stop it.
                if stopping.is_some_and(|at| at.elapsed() > STOPPING) {
                    return Err(CloudError::Cancelled);
                }
                task.report(Stage::Queued(position(&status)));
            }
            // Running, even after a stop: what it makes is paid for, so
            // it is followed and kept.
            Some("IN_PROGRESS") => task.report(Stage::Running),
            _ => {
                // A request that failed is completed too, with its error.
                if let Some(message) = status["error"].as_str() {
                    if stopping.is_some() {
                        return Err(CloudError::Cancelled);
                    }
                    let kind = status["error_type"].as_str().unwrap_or_default();
                    return Err(failure(500, kind, message.to_owned()));
                }
                break;
            }
        }
    }

    // Made, and paid for: asked for again while it doesn't come.
    let made = Instant::now();
    let mut failures = 0;
    loop {
        match http::look(&response_url, &headers, made.elapsed() < FRESH) {
            http::Look::Answer(result) => return Ok(result),
            http::Look::Refused(reply) => return Err(error(&reply)),
            http::Look::Again(why) if made.elapsed() > RESULT_PATIENCE => {
                return Err(CloudError::Failed(format!(
                    "{NAME} made it but did not hand it over (request {id}): {why}"
                )));
            }
            http::Look::Again(_) => {
                failures += 1;
                std::thread::sleep(http::backoff(POLL, failures));
            }
        }
    }
}

/// What the last failed looks were, if the last looks failed.
fn lately(why: &str, failures: u32) -> String {
    if failures == 0 {
        String::new()
    } else {
        format!(" (lately: {why})")
    }
}

/// How many are ahead of it, when fal says.
fn position(json: &Value) -> Option<u32> {
    json["queue_position"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
}

/// Sleep for `pause`; whether the user stopped it meanwhile, once
/// `stop_from` (none: not watched).
fn wait(task: &GenerationTask, pause: Duration, stop_from: Option<Instant>) -> bool {
    let stopped = || task.cancelled() && stop_from.is_some_and(|at| Instant::now() >= at);
    let until = Instant::now() + pause;
    while Instant::now() < until {
        if stopped() {
            return true;
        }
        std::thread::sleep(TICK);
    }
    stopped()
}

enum Cancel {
    Stopped,
    /// Made before the cancel arrived: it is paid for, so it is kept.
    Made,
    /// fal will try: a queued request is dropped, a running one may still
    /// be made.
    Asked,
    /// fal couldn't be told, or wouldn't say; it goes on as it was.
    Unsent,
}

fn cancel(url: &str, headers: &[(&str, &str)]) -> Cancel {
    let Ok(reply) = http::put(url, headers) else {
        return Cancel::Unsent;
    };
    let json: Value = serde_json::from_slice(&reply.body).unwrap_or(Value::Null);
    match json["status"].as_str() {
        Some("ALREADY_COMPLETED") => Cancel::Made,
        // Not one fal has: unknown, so nothing runs.
        _ if reply.status == 404 => Cancel::Stopped,
        // "CANCELLATION_REQUESTED", or nothing said: not stopped yet.
        _ if reply.ok() => Cancel::Asked,
        _ => Cancel::Unsent,
    }
}
