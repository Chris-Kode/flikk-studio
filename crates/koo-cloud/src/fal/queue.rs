//! fal's queue: a request is submitted, followed while it waits and runs,
//! and its result read once it has one. A request can be cancelled until
//! it is made; fal answers `ALREADY_COMPLETED` after that, and what was
//! made is kept.

use super::{NAME, auth, error, failure};
use crate::http;
use koo_core::cloud::{CloudError, CloudResult, GenerationTask, Stage};
use serde_json::Value;
use std::time::{Duration, Instant};

const QUEUE: &str = "https://queue.fal.run";

/// Between looks at where a request is.
const POLL: Duration = Duration::from_secs(3);

/// Between looks at whether the user stopped it, while waiting to poll.
const TICK: Duration = Duration::from_millis(250);

/// The longest a request is followed, queue and all.
const PATIENCE: Duration = Duration::from_secs(2 * 60 * 60);

/// Submit `input` to `endpoint` and follow it until it is made; its result.
/// [`CloudError::Cancelled`] when `task` was cancelled in time.
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
    loop {
        if wait(task) {
            match cancel(&cancel_url, &headers)? {
                Cancel::Stopped => return Err(CloudError::Cancelled),
                Cancel::Made => break,
            }
        }
        if started.elapsed() > PATIENCE {
            let _ = cancel(&cancel_url, &headers);
            return Err(CloudError::Failed(format!(
                "{NAME} took over two hours, so it was stopped"
            )));
        }
        let reply = http::get(&status_url, &headers)?;
        if !reply.ok() {
            return Err(error(&reply));
        }
        let status = reply.json()?;
        match status["status"].as_str() {
            Some("IN_QUEUE") => task.report(Stage::Queued(position(&status))),
            Some("IN_PROGRESS") => task.report(Stage::Running),
            Some("COMPLETED") => {
                // A request that failed is completed too, with its error.
                if let Some(message) = status["error"].as_str() {
                    let kind = status["error_type"].as_str().unwrap_or_default();
                    return Err(failure(500, kind, message.to_owned()));
                }
                break;
            }
            _ => {}
        }
    }

    let reply = http::get(&response_url, &headers)?;
    if !reply.ok() {
        return Err(error(&reply));
    }
    reply.json()
}

/// How many are ahead of it, when fal says.
fn position(json: &Value) -> Option<u32> {
    json["queue_position"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
}

/// Sleep until the next poll; whether the user stopped it meanwhile.
fn wait(task: &GenerationTask) -> bool {
    let until = Instant::now() + POLL;
    while Instant::now() < until {
        if task.cancelled() {
            return true;
        }
        std::thread::sleep(TICK);
    }
    task.cancelled()
}

enum Cancel {
    Stopped,
    /// Made before the cancel arrived: it is paid for, so it is kept.
    Made,
}

fn cancel(url: &str, headers: &[(&str, &str)]) -> CloudResult<Cancel> {
    let reply = http::put(url, headers)?;
    let json: Value = serde_json::from_slice(&reply.body).unwrap_or(Value::Null);
    if json["status"].as_str() == Some("ALREADY_COMPLETED") {
        return Ok(Cancel::Made);
    }
    if reply.ok() || reply.status == 404 {
        return Ok(Cancel::Stopped);
    }
    Err(error(&reply))
}
