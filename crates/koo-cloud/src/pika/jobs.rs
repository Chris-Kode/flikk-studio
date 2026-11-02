//! Pika's media jobs: a request is submitted, then followed while it waits
//! and runs until it is made or has failed. Pika can't stop a job (a
//! running one can't even be deleted), so the task is marked unstoppable
//! once it is taken: it is made, paid for and kept.

use super::{BASE, NAME, error, failure};
use crate::http;
use koo_core::cloud::{CloudError, CloudResult, GenerationTask, Stage};
use serde_json::Value;
use std::time::{Duration, Instant};

/// Between looks at where a job is.
const POLL: Duration = Duration::from_secs(3);

/// The longest a job is followed, queue and all.
const PATIENCE: Duration = Duration::from_secs(2 * 60 * 60);

/// Submit `input` to the endpoint at `path` (after `/v1/media/`) and follow
/// it until it is made; the job, with its `output`.
pub fn run(key: &str, path: &str, input: &Value, task: &GenerationTask) -> CloudResult<Value> {
    let headers = [("x-api-key", key)];
    task.check()?;
    // Not retried on a server error: it may have been taken, and charged.
    let reply = http::post_json(&format!("{BASE}/v1/media/{path}"), &headers, input, false)?;
    if !reply.ok() {
        return Err(error(&reply));
    }
    let mut job = reply.json()?;
    let id = job["id"]
        .as_str()
        .ok_or_else(|| CloudError::Failed(format!("{NAME} did not take the request")))?
        .to_owned();
    task.unstoppable();
    let status_url = format!("{BASE}/v1/media/jobs/{id}");
    let started = Instant::now();
    loop {
        match job["status"].as_str() {
            Some("completed") => return Ok(job),
            Some("failed") => return Err(failed(&job)),
            Some("running") => task.report(Stage::Running),
            _ => task.report(Stage::Queued(None)),
        }
        if started.elapsed() > PATIENCE {
            return Err(CloudError::Failed(format!(
                "{NAME} took over two hours; it stopped being followed"
            )));
        }
        std::thread::sleep(POLL);
        let reply = http::get(&status_url, &headers)?;
        if !reply.ok() {
            return Err(error(&reply));
        }
        job = reply.json()?;
    }
}

/// Why a failed job failed.
fn failed(job: &Value) -> CloudError {
    let error = &job["error"];
    let message = error["message"]
        .as_str()
        .filter(|m| !m.is_empty())
        .map_or_else(|| format!("{NAME} could not make it"), str::to_owned);
    failure(200, error["code"].as_str().unwrap_or_default(), message)
}
