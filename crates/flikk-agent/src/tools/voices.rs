//! Voices for voiceovers (the Generate group). `find_voices` lists the
//! account's voices, the user's own first, or searches the provider's shared
//! Voice Library; both are free. `add_voice` adds a shared one to the
//! account and `design_voice` makes one from a description; each takes a
//! voice slot or credits, so it waits for the user's approval unless they
//! allowed generating. Either way the voice is kept in the user's library,
//! where every project finds it. Cloning a voice is the user's alone: it
//! needs the consent of whoever it belongs to.
//!
//! Where each voice listed can be heard is remembered, so an approval card
//! plays it before the user decides.

use super::{Done, Job, JobDone, Step, ToolCtx};
use crate::args::{Args, ToolResultOf, fail};
use flikk_core::cloud::{
    self, Capability, GenerationService, Voice, VoiceDesign, VoiceOrigin, VoiceQuery,
};
use flikk_core::library::{self, Library, SavedVoice};
use flikk_core::model::MediaItem;
use flikk_core::services::MediaService;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

/// Characters of a description listed for each voice.
const ABOUT: usize = 160;

/// Where the provider says each voice listed can be heard, by its id.
static SAMPLES: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(Default::default);

fn note_samples<'a>(voices: impl IntoIterator<Item = &'a Voice>) {
    let Ok(mut samples) = SAMPLES.lock() else {
        return;
    };
    for v in voices {
        if let Some(url) = &v.sample {
            samples.insert(v.id.clone(), url.clone());
        }
    }
}

/// `args` of an approval, with where `voice` can be heard when it was
/// listed: the card plays it.
pub fn with_sample(mut args: Value, voice: &str) -> Value {
    let url = SAMPLES.lock().ok().and_then(|s| s.get(voice).cloned());
    if let (Some(url), Some(map)) = (url, args.as_object_mut()) {
        map.insert("sample".into(), json!(url));
    }
    args
}

pub fn find_voices(ctx: &mut ToolCtx, args: &Args) -> ToolResultOf<Step> {
    let shared = args.opt_bool("library")?.unwrap_or(false);
    let label = |key: &str, allowed: &[&str]| -> ToolResultOf<Option<String>> {
        match args.opt_str(key)?.map(str::trim).filter(|v| !v.is_empty()) {
            Some(v) if allowed.is_empty() || allowed.contains(&v) => Ok(Some(v.to_owned())),
            Some(v) => fail(format!(
                "`{key}` is one of {}, not `{v}`",
                allowed.join(", ")
            )),
            None => Ok(None),
        }
    };
    let query = VoiceQuery {
        text: args.opt_str("text")?.unwrap_or_default().trim().to_owned(),
        gender: label("gender", &cloud::VOICE_GENDERS)?,
        age: label("age", &cloud::VOICE_AGES)?,
        language: label("language", &[])?,
        use_case: label("use_case", &cloud::VOICE_USES)?,
    };
    let limit = args.opt_f64("limit")?.map_or(20, |n| n.max(1.0) as usize);
    let generation = voices_service(ctx)?;
    let job: Job = Box::new(move || {
        if shared {
            return match generation.search_voices(&query, 0) {
                Ok(page) => {
                    note_samples(&page.voices);
                    let found: Vec<Value> =
                        page.voices.iter().take(limit).map(shared_json).collect();
                    let summary = format!("found {} shared voices", found.len());
                    JobDone::Done(Done::json(
                        summary,
                        &json!({
                            "voices": found,
                            "note": "Shared voices read only once added to the account: add_voice, which waits for the user's approval and takes one of their voice slots. A `rate` above 1 costs that many times the credits."
                        }),
                    ))
                }
                Err(e) => JobDone::Done(Done::error(super::generate::error_text(&e))),
            };
        }
        let mut voices = match generation.voices() {
            Ok(voices) => voices,
            Err(e) => return JobDone::Done(Done::error(super::generate::error_text(&e))),
        };
        let lib = Library::load();
        let provider = generation.voice_provider().unwrap_or_default();
        let saved = |v: &Voice| lib.voice_of(&provider, &v.id).cloned();
        note_samples(&voices);
        voices.retain(|v| query.fits(v));
        // The user's library first, then voices they made, then the rest.
        voices.sort_by_key(|v| (saved(v).is_none(), !v.mine()));
        let found: Vec<Value> = voices
            .iter()
            .take(limit)
            .map(|v| account_json(v, saved(v).as_ref()))
            .collect();
        let summary = format!("found {} voices", found.len());
        JobDone::Done(Done::json(summary, &json!({ "voices": found })))
    });
    Ok(Step::Job(job))
}

fn about(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let short: String = text.chars().take(ABOUT).collect();
    Some(if short.len() < text.len() {
        short + "…"
    } else {
        short
    })
}

fn account_json(v: &Voice, saved: Option<&SavedVoice>) -> Value {
    let mut value = json!({
        "voice": v.id,
        "name": v.name,
        "traits": v.traits,
    });
    if let Some(about) = about(&v.description) {
        value["about"] = json!(about);
    }
    if v.mine() {
        value["mine"] = json!(true);
    }
    if let Some(saved) = saved {
        value["in_library"] = json!(true);
        value["stability"] = json!(saved.stability);
        value["similarity"] = json!(saved.similarity);
    }
    value
}

fn shared_json(v: &Voice) -> Value {
    let mut value = json!({
        "voice": v.id,
        "owner": v.owner,
        "name": v.name,
        "traits": v.traits,
    });
    if let Some(about) = about(&v.description) {
        value["about"] = json!(about);
    }
    if let Some(language) = &v.language {
        value["language"] = json!(language);
    }
    if let Some(rate) = v.rate {
        value["rate"] = json!(rate);
    }
    if let Some(days) = v.notice_days {
        value["notice_days"] = json!(days);
    }
    value
}

/// The service, when a provider with a key does voiceovers.
fn voices_service(ctx: &ToolCtx) -> ToolResultOf<Arc<dyn GenerationService>> {
    let Some(generation) = ctx.services.generation.clone() else {
        return fail("Generation is not available in this build.");
    };
    if generation.provider_for(Capability::Voiceover).is_none() {
        return fail(format!(
            "No provider with an API key is set for {} in Settings → Integrations. Tell the user; don't retry.",
            Capability::Voiceover.label().to_lowercase()
        ));
    }
    Ok(generation)
}

// ------------------------------------------------------------------ add

/// A shared voice to add: its id, its owner's, and the name to give it.
pub struct Adding {
    pub voice: String,
    pub owner: String,
    pub name: String,
}

pub fn parse_add(args: &Args) -> ToolResultOf<Adding> {
    let id = |key: &str| -> ToolResultOf<String> {
        let value = args.str(key)?.trim();
        if value.is_empty() || !value.chars().all(|c| c.is_ascii_alphanumeric()) {
            return fail(format!("`{key}` is the id find_voices gave"));
        }
        Ok(value.to_owned())
    };
    let voice = id("voice")?;
    let owner = id("owner")?;
    let name = args.opt_str("name")?.map(str::trim).unwrap_or_default();
    Ok(Adding {
        name: if name.is_empty() {
            "Shared voice".into()
        } else {
            name.to_owned()
        },
        voice,
        owner,
    })
}

pub fn add_summary(adding: &Adding) -> String {
    format!("Add the voice “{}” from the Voice Library", adding.name)
}

pub fn add_json(adding: &Adding) -> Value {
    json!({ "voice": adding.voice, "owner": adding.owner, "name": adding.name })
}

pub fn add_from_json(value: &Value) -> Option<Adding> {
    let text = |key: &str| value.get(key)?.as_str().map(str::to_owned);
    Some(Adding {
        voice: text("voice")?,
        owner: text("owner")?,
        name: text("name")?,
    })
}

/// Add the shared voice to the account and keep it in the library, with a
/// sample. What to tell the model.
pub fn add(
    generation: &dyn GenerationService,
    media: Option<&dyn MediaService>,
    adding: &Adding,
) -> Result<String, String> {
    let shared = Voice {
        id: adding.voice.clone(),
        name: adding.name.clone(),
        owner: Some(adding.owner.clone()),
        ..Voice::default()
    };
    let added = generation
        .add_voice(&shared, &adding.name)
        .map_err(|e| super::generate::error_text(&e))?;
    // The account's list has its sample and labels now.
    let listed = generation
        .voices()
        .ok()
        .and_then(|all| all.into_iter().find(|v| v.id == added.id));
    let voice = listed.unwrap_or(added);
    let sample = sample_of(generation, media, &voice);
    let origin = VoiceOrigin::Shared {
        owner: adding.owner.clone(),
        name: adding.name.clone(),
    };
    keep(generation, &voice, origin, sample.as_ref())?;
    Ok(format!(
        "voice {} (“{}”) is on the account and in the user's library; read with it in generate_audio",
        voice.id, voice.name
    ))
}

// --------------------------------------------------------------- design

pub fn parse_design(args: &Args) -> ToolResultOf<(VoiceDesign, String)> {
    let design = VoiceDesign {
        description: args.str("description")?.trim().to_owned(),
        text: args
            .opt_str("text")?
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_owned),
    };
    design
        .check()
        .map_err(|e| crate::args::ToolError(e.to_string()))?;
    let name = args.str("name")?.trim().to_owned();
    if name.is_empty() {
        return fail("`name` is what the user will call the voice");
    }
    Ok((design, name))
}

pub fn design_summary(design: &VoiceDesign, name: &str) -> String {
    format!("Design the voice “{name}”: {}", design.description)
}

pub fn design_json(design: &VoiceDesign, name: &str) -> Value {
    json!({ "design": design, "name": name })
}

pub fn design_from_json(value: &Value) -> Option<(VoiceDesign, String)> {
    let design = serde_json::from_value(value.get("design")?.clone()).ok()?;
    Some((design, value.get("name")?.as_str()?.to_owned()))
}

/// Design a voice, keep the first of what the provider offers on the
/// account and in the library. What to tell the model.
pub fn design(
    generation: &dyn GenerationService,
    media: Option<&dyn MediaService>,
    design: &VoiceDesign,
    name: &str,
) -> Result<String, String> {
    let designed = generation
        .design_voice(design)
        .map_err(|e| super::generate::error_text(&e))?;
    let preview = designed
        .previews
        .first()
        .ok_or("the provider made no voices")?;
    let voice = generation
        .save_voice(design, preview, name)
        .map_err(|e| super::generate::error_text(&e))?;
    let sample = cloud::sample_item(preview.audio.clone(), media);
    let origin = VoiceOrigin::Designed {
        description: design.description.clone(),
        text: design.text.clone(),
    };
    keep(generation, &voice, origin, Some(&sample))?;
    Ok(format!(
        "voice {} (“{}”) is on the account and in the user's library; read with it in generate_audio. The user can design it again from the Voices dialog to hear other takes",
        voice.id, voice.name
    ))
}

// ------------------------------------------------------------------ jobs

/// With the user's leave (generating allowed): add a shared voice.
pub fn serve_add(ctx: &mut ToolCtx, args: &Args) -> ToolResultOf<Step> {
    let adding = parse_add(args)?;
    let generation = voices_service(ctx)?;
    let media = ctx.services.media.clone();
    let summary = add_summary(&adding);
    let job: Job = Box::new(move || {
        JobDone::Done(match add(generation.as_ref(), media.as_deref(), &adding) {
            Ok(text) => Done::text(summary, text),
            Err(e) => Done::error(e),
        })
    });
    Ok(Step::Job(job))
}

/// With the user's leave (generating allowed): design a voice.
pub fn serve_design(ctx: &mut ToolCtx, args: &Args) -> ToolResultOf<Step> {
    let (asked, name) = parse_design(args)?;
    let generation = voices_service(ctx)?;
    let media = ctx.services.media.clone();
    let summary = design_summary(&asked, &name);
    let job: Job = Box::new(move || {
        JobDone::Done(
            match design(generation.as_ref(), media.as_deref(), &asked, &name) {
                Ok(text) => Done::text(summary, text),
                Err(e) => Done::error(e),
            },
        )
    });
    Ok(Step::Job(job))
}

/// Whether voices can be added and designed now; why not, when not.
pub fn check_available(ctx: &ToolCtx) -> ToolResultOf<()> {
    voices_service(ctx).map(|_| ())
}

fn sample_of(
    generation: &dyn GenerationService,
    media: Option<&dyn MediaService>,
    voice: &Voice,
) -> Option<MediaItem> {
    let path = generation.voice_sample(voice).ok()?;
    Some(cloud::sample_item(path, media))
}

fn keep(
    generation: &dyn GenerationService,
    voice: &Voice,
    origin: VoiceOrigin,
    sample: Option<&MediaItem>,
) -> Result<(), String> {
    let provider = generation
        .voice_provider()
        .ok_or("no provider does voiceovers")?;
    let entry = SavedVoice::new(&provider, voice, origin);
    library::save_voice(&Library::load(), &entry, sample)
        .map(|_| ())
        .map_err(|e| format!("made it, but could not keep it in the library: {e}"))
}
