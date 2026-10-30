//! ElevenLabs (api.elevenlabs.io): transcription with Scribe, music and
//! sound effects. Every call sends the key as `xi-api-key`.

mod music;
mod sound;
mod transcribe;

use crate::http::{self, Reply};
use crate::{Account, Api, CloudModel, Offer, Provider};
use koo_core::cloud::{Capability, CloudError, CloudResult, GenerateRequest, Heard, SpeechRequest};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const API: &str = "https://api.elevenlabs.io";
const NAME: &str = "ElevenLabs";

pub static PROVIDER: Provider = Provider {
    id: "elevenlabs",
    name: NAME,
    keys_url: "https://elevenlabs.io/app/settings/api-keys",
    key_env: "ELEVENLABS_API_KEY",
    offers: &[
        Offer {
            capability: Capability::Transcription,
            models: &[
                CloudModel {
                    id: "scribe_v2",
                    name: "Scribe v2",
                },
                CloudModel {
                    id: "scribe_v2_medical",
                    name: "Scribe v2 Medical",
                },
            ],
        },
        Offer {
            capability: Capability::Music,
            models: &[
                CloudModel {
                    id: "music_v2_5",
                    name: "Music v2.5",
                },
                CloudModel {
                    id: "music_v2",
                    name: "Music v2",
                },
                CloudModel {
                    id: "music_v1",
                    name: "Music v1",
                },
            ],
        },
        Offer {
            capability: Capability::SoundEffects,
            models: &[
                CloudModel {
                    id: "eleven_text_to_sound_v2",
                    name: "Sound Effects v2",
                },
                CloudModel {
                    id: "eleven_text_to_sound_v3",
                    name: "Sound Effects v3",
                },
            ],
        },
    ],
    api: &ElevenLabs,
};

pub struct ElevenLabs;

impl Api for ElevenLabs {
    fn account(&self, key: &str) -> CloudResult<Account> {
        let reply = http::get(
            &format!("{API}/v1/user/subscription"),
            &[("xi-api-key", key)],
        )?;
        if !reply.ok() {
            return match error(&reply) {
                // A key scoped to other endpoints can't read the plan, but
                // it was accepted.
                CloudError::NotAllowed(_) => Ok(Account::default()),
                e => Err(e),
            };
        }
        let json = reply.json()?;
        Ok(Account {
            plan: json["tier"].as_str().map(str::to_owned),
            credits_used: json["character_count"].as_u64(),
            credits_limit: json["character_limit"].as_u64(),
        })
    }

    fn transcribe(
        &self,
        key: &str,
        model: &str,
        request: &SpeechRequest,
        progress: &(dyn Fn(f32) + Sync),
    ) -> CloudResult<Heard> {
        transcribe::run(key, model, request, progress)
    }

    fn generate(
        &self,
        key: &str,
        model: &str,
        request: &GenerateRequest,
        stem: &Path,
    ) -> CloudResult<(PathBuf, Option<u64>)> {
        match request {
            GenerateRequest::Music { .. } => music::run(key, model, request, stem),
            GenerateRequest::SoundEffect { .. } => sound::run(key, model, request, stem),
        }
    }
}

/// What an error reply means. The body is `{"detail": {"type", "code",
/// "status", "message", "data"}}`, a list of validation errors, or a plain
/// message, depending on the endpoint and its age.
fn error(reply: &Reply) -> CloudError {
    let json: Value = serde_json::from_slice(&reply.body).unwrap_or(Value::Null);
    let detail = &json["detail"];
    let codes: Vec<&str> = ["code", "status", "type"]
        .iter()
        .filter_map(|k| detail[*k].as_str())
        .collect();
    let has = |code: &str| codes.contains(&code);
    let message = detail["message"]
        .as_str()
        .or(detail.as_str())
        .map(str::to_owned)
        .or_else(|| {
            detail.as_array().map(|errors| {
                errors
                    .iter()
                    .filter_map(|e| e["msg"].as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            })
        })
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| format!("{NAME} answered {}", reply.status));
    let provider = NAME.to_owned();
    if has("bad_prompt") || has("bad_composition_plan") {
        let data = &detail["data"];
        let suggestion = data["prompt_suggestion"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| {
                let plan = &data["composition_plan_suggestion"];
                (!plan.is_null()).then(|| plan.to_string())
            });
        return CloudError::BadPrompt {
            message,
            suggestion,
        };
    }
    if has("quota_exceeded") || has("insufficient_credits") || reply.status == 402 {
        return CloudError::NoCredits { provider };
    }
    // A key scoped to other endpoints; older replies say so with a 401.
    if has("missing_permissions") || has("insufficient_permissions") {
        return CloudError::NotAllowed(format!("{NAME}: {message}"));
    }
    match reply.status {
        401 => CloudError::InvalidKey { provider },
        403 => CloudError::NotAllowed(format!("{NAME}: {message}")),
        429 => CloudError::Busy { provider },
        _ => CloudError::Failed(format!("{NAME}: {message}")),
    }
}

/// Whether `reply` refused the output format asked for (the plan doesn't
/// include it), so a lesser one may do.
fn format_refused(reply: &Reply) -> bool {
    if reply.ok() || !matches!(reply.status, 400 | 403 | 422) {
        return false;
    }
    let text = String::from_utf8_lossy(&reply.body).to_lowercase();
    text.contains("output_format")
        || text.contains("output format")
        || text.contains("feature_not_available")
        || text.contains("subscription_required")
}

/// Output formats the account's plan refused, so later requests start
/// with one it allows.
struct Formats {
    refused: Mutex<Vec<&'static str>>,
}

impl Formats {
    const fn new() -> Self {
        Self {
            refused: Mutex::new(Vec::new()),
        }
    }

    /// POST `body` to `endpoint` with each of `formats` (best first, none
    /// refused before) until one is allowed; the reply and the format
    /// used. The last format is always tried.
    fn post(
        &self,
        endpoint: &str,
        formats: &[&'static str],
        key: &str,
        body: &Value,
    ) -> CloudResult<(Reply, &'static str)> {
        let last = formats.len() - 1;
        for (i, &format) in formats.iter().enumerate() {
            if i < last && crate::lock(&self.refused).contains(&format) {
                continue;
            }
            let url = format!("{API}{endpoint}?output_format={format}");
            let reply = http::post_json(&url, &[("xi-api-key", key)], body, false)?;
            if i < last && format_refused(&reply) {
                crate::lock(&self.refused).push(format);
                continue;
            }
            if !reply.ok() {
                return Err(error(&reply));
            }
            return Ok((reply, format));
        }
        unreachable!("the last format is always tried")
    }
}

static FORMATS: Formats = Formats::new();

/// Write `bytes` to `<stem>.<ext>` through a `.part` file.
fn write_audio(stem: &Path, format: &str, bytes: &[u8]) -> CloudResult<PathBuf> {
    let ext = format.split('_').next().unwrap_or("mp3");
    let mut name = stem.as_os_str().to_owned();
    name.push(".");
    name.push(ext);
    let path = PathBuf::from(name);
    let part = path.with_extension(format!("{ext}.part"));
    let fail =
        |e: std::io::Error| CloudError::Failed(format!("could not write {}: {e}", path.display()));
    std::fs::write(&part, bytes).map_err(fail)?;
    std::fs::rename(&part, &path).map_err(fail)?;
    Ok(path)
}

/// Credits a reply says it cost.
fn cost(reply: &Reply) -> Option<u64> {
    reply.header("character-cost")?.trim().parse().ok()
}
