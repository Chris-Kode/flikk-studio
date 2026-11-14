//! Pika (dev.pika.art): video and pictures, from the models in
//! [`models`], larger videos, from the operations in [`upscale`], and
//! music and sound effects, from those in [`audio`].
//! Video: Pika's own (Pika H3 Flex, Pika 2.5 with Pikaframes) and the
//! other vendors' Pika hosts (Seedance, Kling, MiniMax, Hailuo, Wan, Happy
//! Horse, Gemini Omni Flash, FLUX, Grok Imagine). Pictures, from a
//! description or from pictures: Nano Banana, GPT Image, Seedream,
//! Ideogram, Grok Imagine, Muse, Recraft. Larger videos: Topaz, and Pika H3
//! Flex's own upscale. Music: Pika Music, Sonilo, MiniMax, Lyria. Sound
//! effects: Pika SFX, Sonilo, ElevenLabs. Every call sends the key as
//! `X-API-Key`. All are made as media jobs ([`jobs`]); what goes in is
//! [`video`]'s, [`image`]'s, [`upscale`]'s or [`audio`]'s, and the files they follow are uploaded to Pika first
//! ([`files`]). See `docs/pika.md`.

mod audio;
mod files;
mod image;
mod jobs;
mod models;
mod upscale;
mod video;

use crate::http::{self, Reply};
use crate::{Account, Api, CloudModel, Offer, Provider};
use flikk_core::cloud::{
    AudioModel, Capability, CloudError, CloudResult, GenerateRequest, GenerationTask, ImageModel,
    UpscaleModel, VideoModel,
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
    offers: &[
        Offer {
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
        },
        Offer {
            capability: Capability::Image,
            // The ids and order of `models::IMAGE`, the default first.
            models: &[
                CloudModel {
                    id: "google/gemini-nano-banana-2.1",
                    name: "Nano Banana 2.1",
                },
                CloudModel {
                    id: "google/gemini-3-pro-image",
                    name: "Nano Banana Pro",
                },
                CloudModel {
                    id: "openai/gpt-image-2",
                    name: "GPT Image 2",
                },
                CloudModel {
                    id: "openai/gpt-image-2.6-flare",
                    name: "GPT Image 2.6 Flare",
                },
                CloudModel {
                    id: "openai/gpt-image-2.6-sunburst",
                    name: "GPT Image 2.6 Sunburst",
                },
                CloudModel {
                    id: "bytedance/seedream-5.5",
                    name: "Seedream 5.5",
                },
                CloudModel {
                    id: "bytedance/seedream-5.0-pro",
                    name: "Seedream 5.0 Pro",
                },
                CloudModel {
                    id: "bytedance/seedream-5.0-lite",
                    name: "Seedream 5.0 Lite",
                },
                CloudModel {
                    id: "ideogram/ideogram-4.5",
                    name: "Ideogram 4.5",
                },
                CloudModel {
                    id: "x-ai/grok-imagine-image-2.0",
                    name: "Grok Imagine Image 2.0",
                },
                CloudModel {
                    id: "meta/muse-image-1.0",
                    name: "Muse Image",
                },
                CloudModel {
                    id: "recraft/recraft-4.1",
                    name: "Recraft 4.1",
                },
            ],
        },
        Offer {
            capability: Capability::Upscale,
            // The ids and order of `upscale::UPSCALE`, the default first.
            models: &[
                CloudModel {
                    id: "topaz/topaz-video-upscale",
                    name: "Topaz Proteus",
                },
                CloudModel {
                    id: "topaz/topaz-video-upscale/starlight-precise-2.6",
                    name: "Topaz Starlight Precise",
                },
                CloudModel {
                    id: "topaz/topaz-video-upscale/starlight-fast-2",
                    name: "Topaz Starlight Fast",
                },
                CloudModel {
                    id: "pika/pika-h3-flex/draft-to-video",
                    name: "Pika H3 Flex Upscale",
                },
            ],
        },
        Offer {
            capability: Capability::Music,
            // The ids and order of `audio::MUSIC`, the default first.
            models: &[
                CloudModel {
                    id: "pika/pika-audio/pika-music",
                    name: "Pika Music",
                },
                CloudModel {
                    id: "sonilo/sonilo-v1.1-music/text-to-music",
                    name: "Sonilo 1.1",
                },
                CloudModel {
                    id: "minimax/minimax-music-3.0/text-to-audio",
                    name: "MiniMax Music 3.0",
                },
                CloudModel {
                    id: "google/lyria-3.5/text-to-audio",
                    name: "Lyria 3.5",
                },
                CloudModel {
                    id: "google/lyria-3-pro/text-to-audio",
                    name: "Lyria 3 Pro",
                },
            ],
        },
        Offer {
            capability: Capability::SoundEffects,
            // The ids and order of `audio::SOUND`, the default first.
            models: &[
                CloudModel {
                    id: "pika/pika-audio/pika-sfx",
                    name: "Pika SFX",
                },
                CloudModel {
                    id: "sonilo/sonilo-v1.1-sfx/sound-effects",
                    name: "Sonilo 1.1 SFX",
                },
                CloudModel {
                    id: "elevenlabs/eleven-text-to-sound-v2/sound-effects",
                    name: "ElevenLabs Sound Effects v2",
                },
            ],
        },
    ],
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
        let missing = || CloudError::Unavailable(format!("{NAME} has no model {model}"));
        match request.capability() {
            Capability::Image => {
                let model = models::find_image(model).ok_or_else(missing)?;
                image::run(key, model, request, stem, task)
            }
            Capability::Upscale => {
                let model = upscale::find(model).ok_or_else(missing)?;
                upscale::run(key, model, request, stem, task)
            }
            Capability::Music | Capability::SoundEffects => {
                let model = audio::find(model).ok_or_else(missing)?;
                audio::run(key, model, request, stem, task)
            }
            Capability::Video => {
                let model = models::find(model).ok_or_else(missing)?;
                video::run(key, model, request, stem, task)
            }
            Capability::Transcription | Capability::Voiceover => {
                Err(CloudError::Unavailable(format!(
                    "{NAME} doesn't make {}",
                    request.capability().label().to_lowercase()
                )))
            }
        }
    }

    fn video_model(&self, model: &str) -> Option<&'static VideoModel> {
        models::find(model).map(|m| &m.spec)
    }

    fn image_model(&self, model: &str) -> Option<&'static ImageModel> {
        models::find_image(model).map(|m| &m.spec)
    }

    fn upscale_model(&self, model: &str) -> Option<&'static UpscaleModel> {
        upscale::find(model).map(|m| &m.spec)
    }

    fn audio_model(&self, model: &str) -> Option<&'static AudioModel> {
        audio::find(model).map(|m| &m.spec)
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
