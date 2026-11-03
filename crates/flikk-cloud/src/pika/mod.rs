//! Pika (dev.pika.art): video, from the models in [`models`]: Pika's own
//! (Pika H3 Flex, Pika 2.5 with Pikaframes) and the other vendors' Pika
//! hosts (Seedance, Kling, MiniMax, Hailuo, Wan, Happy Horse, Gemini Omni
//! Flash, FLUX, Grok Imagine). Every call sends the key as
//! `X-API-Key`. A video is made as a media job ([`jobs`]); what goes in is
//! [`video`]'s, and the files it follows are uploaded to Pika first
//! ([`files`]). See `docs/pika.md`.

mod files;
mod jobs;
mod models;
mod video;

use crate::http::{self, Reply};
use crate::{Account, Api, CloudModel, Offer, Provider};
use flikk_core::cloud::{
    Capability, CloudError, CloudResult, GenerateRequest, GenerationTask, VideoModel,
};
use serde_json::Value;
use std::path::{Path, PathBuf};

const NAME: &str = "Pika";

const BASE: &str = "https://api.dev.pika.art";

pub static PROVIDER: Provider = Provider {
    id: "pika",
    name: NAME,
    keys_url: "https://dev.pika.art/keys",
    key_env: "PIKA_API_KEY",
    offers: &[Offer {
        capability: Capability::Video,
        // The ids and order of `models::VIDEO`, the default first.
        models: &[
            CloudModel {
                id: "pika/pika-h3-flex",
                name: "Pika H3 Flex",
            },
            CloudModel {
                id: "pika/pika-2.5",
                name: "Pika 2.5",
            },
            CloudModel {
                id: "bytedance/seedance-2.0",
                name: "Seedance 2.0",
            },
            CloudModel {
                id: "bytedance/seedance-2.0-fast",
                name: "Seedance 2.0 Fast",
            },
            CloudModel {
                id: "bytedance/seedance-2.0-mini",
                name: "Seedance 2.0 Mini",
            },
            CloudModel {
                id: "bytedance/seedance-2.5",
                name: "Seedance 2.5",
            },
            CloudModel {
                id: "kling/kling-4.0",
                name: "Kling 4.0",
            },
            CloudModel {
                id: "kling/kling-3.0",
                name: "Kling 3.0",
            },
            CloudModel {
                id: "kling/kling-3.0-turbo",
                name: "Kling 3.0 Turbo",
            },
            CloudModel {
                id: "minimax/h3",
                name: "MiniMax H3",
            },
            CloudModel {
                id: "minimax/hailuo-2.3",
                name: "Hailuo 2.3",
            },
            CloudModel {
                id: "alibaba/wan3.0-video",
                name: "Wan 3.0",
            },
            CloudModel {
                id: "alibaba/wan3.0-video-prime",
                name: "Wan 3.0 Prime",
            },
            CloudModel {
                id: "alibaba/happyhorse-1.1",
                name: "Happy Horse 1.1",
            },
            CloudModel {
                id: "alibaba/happyhorse-1.0",
                name: "Happy Horse 1.0",
            },
            CloudModel {
                id: "google/gemini-omni-flash",
                name: "Gemini Omni Flash",
            },
            CloudModel {
                id: "black-forest-labs/flux-3-video",
                name: "FLUX 3",
            },
            CloudModel {
                id: "x-ai/grok-imagine-video",
                name: "Grok Imagine Video",
            },
        ],
    }],
    api: &Pika,
};

pub struct Pika;

impl Api for Pika {
    /// What is left to spend: the balance, or what is left of an invoiced
    /// account's limit this cycle. Free to read.
    fn account(&self, key: &str) -> CloudResult<Account> {
        let reply = http::get(&format!("{BASE}/billing/balance"), &[("x-api-key", key)])?;
        if !reply.ok() {
            return Err(error(&reply));
        }
        let json = reply.json()?;
        let postpaid = &json["postpaid"];
        let micro = if postpaid["active"].as_bool() == Some(true) {
            postpaid["cycle"]["remaining_micro_usd"].as_i64()
        } else {
            json["balance_micro_usd"].as_i64()
        };
        let balance = micro.map(|m| {
            let dollars = m as f64 / 1_000_000.0;
            let sign = if dollars < 0.0 { "-" } else { "" };
            format!("{sign}${:.2}", dollars.abs())
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
        let model = models::find(model)
            .ok_or_else(|| CloudError::Unavailable(format!("{NAME} has no model {model}")))?;
        video::run(key, model, request, stem, task)
    }

    fn video_model(&self, model: &str) -> Option<&'static VideoModel> {
        models::find(model).map(|m| &m.spec)
    }
}

/// What an error reply means. Errors before a job exists are
/// `{"message": "..."}`; a submit refused after (the balance, a rate
/// limit) is the failed job, `{"id", "status", "error": {"code",
/// "message"}}`.
fn error(reply: &Reply) -> CloudError {
    let json: Value = serde_json::from_slice(&reply.body).unwrap_or(Value::Null);
    let job = &json["error"];
    let code = job["code"].as_str().unwrap_or_default();
    let message = job["message"]
        .as_str()
        .or(json["message"].as_str())
        .filter(|m| !m.is_empty())
        .map_or_else(
            || format!("{NAME} answered {}", reply.status),
            str::to_owned,
        );
    failure(reply.status, code, message)
}

/// The error for `status`, with Pika's error `code` (empty when there is
/// none), saying `message`.
fn failure(status: u16, code: &str, message: String) -> CloudError {
    let provider = NAME.to_owned();
    let bad = |message| CloudError::BadPrompt {
        message,
        suggestion: None,
    };
    match code {
        "insufficient_balance" | "cycle_limit_exceeded" => {
            return CloudError::NoCredits { provider };
        }
        "membership_required" | "usage_limit_exceeded" => {
            return CloudError::NotAllowed(format!("{NAME}: {message}"));
        }
        "invalid_input" | "content_moderation" => return bad(message),
        "rate_limited" | "provider_unavailable" => return CloudError::Busy { provider },
        "" => {}
        _ => return CloudError::Failed(format!("{NAME}: {message}")),
    }
    match status {
        401 => CloudError::InvalidKey { provider },
        // An inactive key.
        403 => CloudError::NotAllowed(format!("{NAME}: {message}")),
        422 => bad(message),
        429 | 503 => CloudError::Busy { provider },
        _ => CloudError::Failed(format!("{NAME}: {message}")),
    }
}
