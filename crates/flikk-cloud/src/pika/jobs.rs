//! Pika's media jobs: a request is submitted, then followed while it waits
//! and runs until it is made or has failed. Pika can't stop a job (a
//! running one can't even be deleted), so the task is marked unstoppable
//! once it is taken: it is made, paid for and kept.

use super::{BASE, NAME, error, failure};
use crate::http;
use flikk_core::cloud::{CloudError, CloudResult, GenerationTask, Stage};
use serde_json::Value;
use std::time::{Duration, Instant};

/// Between looks at where a job is.
const POLL: Duration = Duration::from_secs(3);

/// The longest a job is followed, queue and all.
const PATIENCE: Duration = Duration::from_secs(2 * 60 * 60);

/// How long a new job may be unknown to its URL.
const FRESH: Duration = Duration::from_secs(60);

/// How long a status this code doesn't know is waited out.
const STRANGE: Duration = Duration::from_secs(10 * 60);

/// Submit `input` to the endpoint at `path` (after `/v1/media/`) and follow
/// it until it is made; the job, with its `output`. A look that fails for a
/// while (no connection, a failing server) doesn't end it: the job goes on
/// at Pika, and is paid for.
pub fn run(key: &str, path: &str, input: &Value, task: &GenerationTask) -> CloudResult<Value> {
    let headers = [("x-api-key", key)];
    task.check()?;
    // Not retried on a server error: it may have been taken, and charged.
    let reply = http::post_json(
        &format!("{BASE}/v1/media/{path}"),
        &headers,
        input,
        false,
        Some(task),
    )?;
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
    let mut failures = 0;
    let mut last_failure = String::new();
    let mut strange: Option<Instant> = None;
    loop {
        match job["status"].as_str() {
            Some("completed") => return Ok(job),
            Some("failed") => return Err(failed(&job)),
            Some("running") => {
                strange = None;
                task.report(Stage::Running);
            }
            Some("queued") => {
                strange = None;
                task.report(Stage::Queued(None));
            }
            // A status Pika didn't document: its end, or one waited out.
            said => {
                let said = said.unwrap_or("no status");
                if http::ended(said) || job["error"].is_object() {
                    return Err(failed(&job));
                }
                if strange.get_or_insert_with(Instant::now).elapsed() > STRANGE {
                    return Err(CloudError::Failed(format!(
                        "{NAME} answered {said} for ten minutes; job {id} stopped being followed"
                    )));
                }
            }
        }
        if started.elapsed() > PATIENCE {
            let lately = if failures == 0 {
                String::new()
            } else {
                format!(" (lately: {last_failure})")
            };
            return Err(CloudError::Failed(format!(
                "{NAME} took over two hours; job {id} stopped being followed{lately}"
            )));
        }
        std::thread::sleep(http::backoff(POLL, failures));
        // Taken: it is made and kept, whatever the task says.
        match http::look(&status_url, &headers, started.elapsed() < FRESH, None) {
            http::Look::Answer(answer) => {
                failures = 0;
                job = answer;
            }
            http::Look::Again(why) => {
                failures += 1;
                last_failure = why;
            }
            http::Look::Refused(reply) => return Err(error(&reply)),
            http::Look::Stopped => {}
        }
        task.looked(failures);
    }
}

/// Why a failed job failed.
fn failed(job: &Value) -> CloudError {
    let error = &job["error"];
    let message = error["message"]
        .as_str()
        .filter(|m| !m.is_empty())
        .map_or_else(
            || match job["status"].as_str() {
                Some(said) if said != "failed" => format!("{NAME} ended the job as {said}"),
                _ => format!("{NAME} could not make it"),
            },
            str::to_owned,
        );
    failure(200, error["code"].as_str().unwrap_or_default(), message)
}
