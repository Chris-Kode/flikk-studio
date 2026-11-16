//! Lip sync on a picture (`/v1/flows/video`): Creatify Aurora animates a
//! picture so it says a sound. The picture and the sound go inline, as
//! base64; the generation is created, then followed until it is completed
//! or failed, and the video it made is fetched from its signed URL.
//! ElevenLabs can't stop a generation, so the task is marked unstoppable
//! once it is taken: it is made, paid for and kept. A failed one is not
//! charged.

use super::{API, NAME, error};
use crate::http;
use base64::Engine as _;
use flikk_core::cloud::{
    CloudError, CloudResult, FaceKind, GenerateRequest, GenerationTask, LipSyncModel, Stage,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The one model offered: it animates a picture, to a voice of any length.
pub(super) static AURORA: LipSyncModel = LipSyncModel {
    name: "Creatify Aurora",
    face: FaceKind::Picture,
    video_seconds: None,
    voice_seconds: None,
    sides: None,
};

/// The most a file sent inline may hold, decoded.
const LARGEST_INLINE: u64 = 25 << 20;

/// Between looks at where a generation is.
const POLL: Duration = Duration::from_secs(3);

/// The longest a generation is followed.
const PATIENCE: Duration = Duration::from_secs(2 * 60 * 60);

/// How long a new generation may be unknown to its URL.
const FRESH: Duration = Duration::from_secs(60);

/// How long a status this code doesn't know is waited out.
const STRANGE: Duration = Duration::from_secs(10 * 60);

/// Make `request`'s picture say its voice with `model` (`creatify-aurora`),
/// into `<stem>.<ext>`.
pub fn run(
    key: &str,
    model: &str,
    request: &GenerateRequest,
    stem: &Path,
    task: &GenerationTask,
) -> CloudResult<(PathBuf, Option<u64>)> {
    let GenerateRequest::LipSync { face, voice } = request else {
        return Err(CloudError::Unavailable(format!(
            "{} only lip-syncs",
            AURORA.name
        )));
    };
    AURORA.check(request)?;
    let picture = picture_mime(&face.path).ok_or_else(|| {
        refuse(format!(
            "{NAME} takes JPEG, PNG, WebP or HEIC pictures; {} is not one",
            face.name
        ))
    })?;
    let sound = sound_mime(&voice.path).ok_or_else(|| {
        refuse(format!(
            "{NAME} takes MP3 or WAV sounds; {} is not one",
            voice.name
        ))
    })?;
    let body = json!({
        "model_id": model,
        "image": inline(&face.path, &face.name, picture)?,
        "audio": inline(&voice.path, &voice.name, sound)?,
        "resolution": "720p",
    });
    task.check()?;
    follow(key, &body, stem, task)
}

/// Create the generation `body` asks for, follow it until it is made, and
/// fetch its video into `<stem>.<ext>`.
fn follow(
    key: &str,
    body: &Value,
    stem: &Path,
    task: &GenerationTask,
) -> CloudResult<(PathBuf, Option<u64>)> {
    let headers = [("xi-api-key", key)];
    // Not retried on a server error: it may have been taken, and charged.
    let reply = http::post_json(
        &format!("{API}/v1/flows/video"),
        &headers,
        body,
        false,
        Some(task),
    )?;
    if !reply.ok() {
        return Err(error(&reply));
    }
    let mut generation = reply.json()?;
    let id = generation["id"]
        .as_str()
        .ok_or_else(|| CloudError::Failed(format!("{NAME} did not take the request")))?
        .to_owned();
    task.unstoppable();
    let status_url = format!("{API}/v1/flows/video/{id}");
    let started = Instant::now();
    let mut failures = 0;
    let mut last_failure = String::new();
    let mut strange: Option<Instant> = None;
    loop {
        match generation["status"].as_str() {
            Some("completed") => break,
            Some("failed") => return Err(failed(&generation)),
            Some("generating") => {
                strange = None;
                task.report(Stage::Running);
            }
            Some("pending") => {
                strange = None;
                task.report(Stage::Queued(None));
            }
            // A status ElevenLabs didn't document: its end, or one waited
            // out.
            said => {
                let said = said.unwrap_or("no status");
                if http::ended(said) {
                    return Err(failed(&generation));
                }
                if strange.get_or_insert_with(Instant::now).elapsed() > STRANGE {
                    return Err(CloudError::Failed(format!(
                        "{NAME} answered {said} for ten minutes; generation {id} stopped being followed"
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
                "{NAME} took over two hours; generation {id} stopped being followed{lately}"
            )));
        }
        std::thread::sleep(http::backoff(POLL, failures));
        // Taken: it is made and kept, whatever the task says.
        match http::look(&status_url, &headers, started.elapsed() < FRESH, None) {
            http::Look::Answer(answer) => {
                failures = 0;
                generation = answer;
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
    let url = generation["content_url"]
        .as_str()
        .ok_or_else(|| CloudError::Failed(format!("{NAME} sent no video back")))?;
    let extension = match generation["content_mime_type"].as_str() {
        Some("video/webm") => "webm",
        Some("video/quicktime") => "mov",
        _ => "mp4",
    };
    let mut name = stem.as_os_str().to_owned();
    name.push(".");
    name.push(extension);
    let path = PathBuf::from(name);
    task.report(Stage::Downloading(0.0));
    // A signed URL of its own: the key doesn't go with it.
    http::download(url, &path, &|f| task.report(Stage::Downloading(f)))?;
    Ok((path, None))
}

/// Why a failed generation failed: a refused input, an account that
/// couldn't be charged, or the model's own failure.
fn failed(generation: &Value) -> CloudError {
    let message = generation["error_message"]
        .as_str()
        .filter(|m| !m.is_empty())
        .map_or_else(|| format!("{NAME} could not make it"), str::to_owned);
    match generation["failure_reason"].as_str() {
        Some("moderated" | "invalid_parameters") => refuse(message),
        Some("charging_failed") => CloudError::NoCredits {
            provider: NAME.to_owned(),
        },
        _ => CloudError::Failed(format!("{NAME}: {message}")),
    }
}

/// The file at `path` (what the pool calls `name`) as an inline reference
/// of type `mime`, when it is small enough to send that way.
fn inline(path: &Path, name: &str, mime: &str) -> CloudResult<Value> {
    crate::check_upload(path, name, LARGEST_INLINE, NAME)?;
    let bytes = std::fs::read(path)
        .map_err(|e| CloudError::Failed(format!("could not read {name}: {e}")))?;
    Ok(json!({
        "type": "inline_base64",
        "content_base64": base64::engine::general_purpose::STANDARD.encode(bytes),
        "mime_type": mime,
    }))
}

/// The type of a picture ElevenLabs takes, by its extension.
fn picture_mime(path: &Path) -> Option<&'static str> {
    match http::media_mime(path) {
        mime @ ("image/jpeg" | "image/png" | "image/webp" | "image/heic" | "image/heif") => {
            Some(mime)
        }
        _ => None,
    }
}

/// The type of a sound ElevenLabs takes, by its extension: MP3 or WAV.
fn sound_mime(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "mp3" => Some("audio/mpeg"),
        "wav" => Some("audio/wav"),
        _ => None,
    }
}

fn refuse(message: String) -> CloudError {
    CloudError::BadPrompt {
        message,
        suggestion: None,
    }
}
