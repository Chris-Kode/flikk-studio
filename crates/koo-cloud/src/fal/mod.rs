//! fal (fal.ai): video, from ByteDance's Seedance models for now. Every
//! call sends the key as `Authorization: Key <key>`. A video is made on
//! fal's queue ([`queue`]); what goes in is [`video`]'s, and the files it
//! follows are uploaded to fal's storage first ([`files`]).

mod files;
mod queue;
mod video;

use crate::http::{self, Reply};
use crate::{Account, Api, CloudModel, Offer, Provider};
use koo_core::cloud::{
    Capability, CloudError, CloudResult, GenerateRequest, GenerationTask, ReferenceLimits,
    VideoModel,
};
use serde_json::Value;
use std::path::{Path, PathBuf};

const NAME: &str = "fal";

pub static PROVIDER: Provider = Provider {
    id: "fal",
    name: NAME,
    keys_url: "https://fal.ai/dashboard/keys",
    key_env: "FAL_KEY",
    offers: &[Offer {
        capability: Capability::Video,
        // Each id is the start of its endpoints: `<id>/text-to-video`,
        // `<id>/image-to-video`, `<id>/reference-to-video`.
        models: &[
            CloudModel {
                id: "bytedance/seedance-2.0",
                name: "Seedance 2.0",
            },
            CloudModel {
                id: "bytedance/seedance-2.0/fast",
                name: "Seedance 2.0 Fast",
            },
            CloudModel {
                id: "bytedance/seedance-2.5",
                name: "Seedance 2.5",
            },
        ],
    }],
    api: &Fal,
};

const ASPECTS: &[&str] = &["16:9", "9:16", "1:1", "4:3", "3:4", "21:9"];

const SEEDANCE_2: VideoModel = VideoModel {
    name: "Seedance 2.0",
    seconds: (4, 15),
    aspects: ASPECTS,
    resolutions: &["720p", "480p"],
    frame_sets_aspect: false,
    last_frame: true,
    references: Some(ReferenceLimits {
        images: 9,
        videos: 3,
        sounds: 3,
        files: 12,
        seconds: 15.0,
    }),
    audio: true,
};

const SEEDANCE_2_FAST: VideoModel = VideoModel {
    name: "Seedance 2.0 Fast",
    ..SEEDANCE_2
};

const SEEDANCE_2_5: VideoModel = VideoModel {
    name: "Seedance 2.5",
    seconds: (4, 30),
    aspects: ASPECTS,
    resolutions: &["720p", "1080p", "480p"],
    frame_sets_aspect: true,
    last_frame: true,
    references: Some(ReferenceLimits {
        images: 30,
        videos: 10,
        sounds: 10,
        files: 50,
        seconds: 30.0,
    }),
    audio: true,
};

pub struct Fal;

impl Api for Fal {
    /// A key that may read billing shows what is left on the account; one
    /// that may not was still accepted.
    fn account(&self, key: &str) -> CloudResult<Account> {
        let reply = http::get(
            "https://api.fal.ai/v1/account/billing?expand=credits",
            &[("authorization", &auth(key))],
        )?;
        if !reply.ok() {
            return match error(&reply) {
                CloudError::NotAllowed(_) => Ok(Account::default()),
                e => Err(e),
            };
        }
        let json = reply.json()?;
        let credits = &json["credits"];
        let balance = credits["current_balance"].as_f64().map(|amount| {
            match credits["currency"].as_str().unwrap_or("USD") {
                "USD" => format!("${amount:.2}"),
                currency => format!("{amount:.2} {currency}"),
            }
        });
        Ok(Account {
            balance,
            ..Account::default()
        })
    }

    fn generate(
        &self,
        key: &str,
        model: &str,
        request: &GenerateRequest,
        stem: &Path,
        task: &GenerationTask,
    ) -> CloudResult<(PathBuf, Option<u64>)> {
        let spec = self
            .video_model(model)
            .ok_or_else(|| CloudError::Unavailable(format!("{NAME} has no model {model}")))?;
        video::run(key, model, spec, request, stem, task)
    }

    fn video_model(&self, model: &str) -> Option<&'static VideoModel> {
        match model {
            "bytedance/seedance-2.0" => Some(&SEEDANCE_2),
            "bytedance/seedance-2.0/fast" => Some(&SEEDANCE_2_FAST),
            "bytedance/seedance-2.5" => Some(&SEEDANCE_2_5),
            _ => None,
        }
    }
}

fn auth(key: &str) -> String {
    format!("Key {key}")
}

/// What an error reply means. The body is `{"detail": "..."}`, a list of
/// validation errors (`{"detail": [{"loc", "msg", "type"}]}`),
/// `{"detail": {"type", ...}}`, or (the platform API's)
/// `{"error": {"type", "message"}}`; the kind of error may also come in
/// `error_type` or the `x-fal-error-type` header.
fn error(reply: &Reply) -> CloudError {
    let json: Value = serde_json::from_slice(&reply.body).unwrap_or(Value::Null);
    let detail = &json["detail"];
    let platform = &json["error"];
    let kind = reply
        .header("x-fal-error-type")
        .or(json["error_type"].as_str())
        .or(detail["type"].as_str())
        .or(platform["type"].as_str())
        .unwrap_or_default()
        .to_owned();
    let message = detail
        .as_str()
        .or(detail["message"].as_str())
        .or(detail["msg"].as_str())
        .or(platform["message"].as_str())
        .map(str::to_owned)
        .or_else(|| detail.as_array().map(|errors| validation(errors)))
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| format!("{NAME} answered {}", reply.status));
    failure(reply.status, &kind, message)
}

/// Each validation error as "field: why".
fn validation(errors: &[Value]) -> String {
    errors
        .iter()
        .filter_map(|e| {
            let msg = e["msg"].as_str()?;
            let field = e["loc"]
                .as_array()
                .and_then(|loc| loc.last())
                .and_then(Value::as_str);
            Some(match field {
                Some(field) if field != "body" => format!("{field}: {msg}"),
                _ => msg.to_owned(),
            })
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// The error for `status`, of `kind` as fal names it, saying `message`.
fn failure(status: u16, kind: &str, message: String) -> CloudError {
    let provider = NAME.to_owned();
    let lower = message.to_lowercase();
    if status == 401 || kind == "rejected_key" {
        return CloudError::InvalidKey { provider };
    }
    if status == 402 || lower.contains("balance") || lower.contains("exhausted") {
        return CloudError::NoCredits { provider };
    }
    if kind.contains("content_policy") || lower.contains("content policy") {
        return CloudError::BadPrompt {
            message,
            suggestion: None,
        };
    }
    match status {
        403 => CloudError::NotAllowed(format!("{NAME}: {message}")),
        422 => CloudError::BadPrompt {
            message,
            suggestion: None,
        },
        429 => CloudError::Busy { provider },
        _ => CloudError::Failed(format!("{NAME}: {message}")),
    }
}
