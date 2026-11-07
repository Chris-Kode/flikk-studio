//! Voices: the account's own and the default ones (`GET /v2/voices`), the
//! shared library (`GET /v1/shared-voices`, `POST /v1/similar-voices`),
//! adding a shared one (`POST /v1/voices/add/{owner}/{voice}`), cloning
//! from recordings (`POST /v1/voices/add`), renaming and deleting.

use super::{API, error};
use crate::http;
use flikk_core::cloud::{CloudError, CloudResult, Voice, VoiceClone, VoicePage, VoiceQuery};
use serde_json::{Value, json};
use std::path::Path;

/// Of the voice's labels, the ones that say what it is like, in this order.
const TRAITS: [&str; 5] = ["gender", "age", "accent", "descriptive", "use_case"];
/// Pages of 100 read at most, for an account with a very long list.
const PAGES: usize = 20;
/// Shared voices per page.
const SHARED_PAGE: usize = 30;
/// Shared voices like a recording, at most.
const SIMILAR: usize = 20;

pub fn list(key: &str) -> CloudResult<Vec<Voice>> {
    let mut voices = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..PAGES {
        let mut url = format!("{API}/v2/voices?page_size=100&include_total_count=false");
        if let Some(token) = &token {
            url.push_str("&next_page_token=");
            url.push_str(&query_value(token));
        }
        let reply = http::get(&url, &[("xi-api-key", key)])?;
        if !reply.ok() {
            return Err(error(&reply));
        }
        let json = reply.json()?;
        let page = json["voices"].as_array().into_iter().flatten();
        voices.extend(page.filter_map(voice));
        token = json["next_page_token"].as_str().map(str::to_owned);
        if !json["has_more"].as_bool().unwrap_or(false) || token.is_none() {
            break;
        }
    }
    Ok(voices)
}

/// One of the account's voices, as `/v2/voices` and the calls that make
/// one answer.
pub(super) fn voice(json: &Value) -> Option<Voice> {
    let text = |key: &str| json[key].as_str().map(str::to_owned);
    let labels = &json["labels"];
    let language = labels["language"].as_str().map(str::to_owned).or_else(|| {
        json["verified_languages"][0]["language"]
            .as_str()
            .map(str::to_owned)
    });
    Some(Voice {
        id: text("voice_id")?,
        name: text("name")?,
        category: text("category").unwrap_or_default(),
        traits: traits(TRAITS.iter().filter_map(|t| labels[*t].as_str())),
        sample: text("preview_url"),
        description: text("description").unwrap_or_default(),
        language,
        ..Voice::default()
    })
}

/// A voice of the shared library: its labels are fields of its own.
fn shared(json: &Value) -> Option<Voice> {
    let text = |key: &str| {
        json[key]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    Some(Voice {
        id: text("voice_id")?,
        name: text("name")?,
        category: text("category").unwrap_or_default(),
        traits: traits(TRAITS.iter().filter_map(|t| json[*t].as_str())),
        sample: text("preview_url"),
        description: text("description").unwrap_or_default(),
        owner: Some(text("public_owner_id")?),
        rate: json["rate"].as_f64().map(|r| r as f32).filter(|r| *r > 1.0),
        notice_days: json["notice_period"]
            .as_u64()
            .filter(|d| *d > 0)
            .map(|d| d as u32),
        language: text("language"),
    })
}

fn traits<'a>(labels: impl Iterator<Item = &'a str>) -> Vec<String> {
    labels
        .filter(|t| !t.is_empty())
        .map(|t| t.replace('_', " "))
        .collect()
}

pub fn search(key: &str, query: &VoiceQuery, page: usize) -> CloudResult<VoicePage> {
    let mut url = format!("{API}/v1/shared-voices?page_size={SHARED_PAGE}&page={page}");
    let mut add = |name: &str, value: &str| {
        if !value.trim().is_empty() {
            url.push_str(&format!("&{name}={}", query_value(value.trim())));
        }
    };
    add("search", &query.text);
    add("gender", query.gender.as_deref().unwrap_or_default());
    add("age", query.age.as_deref().unwrap_or_default());
    add("language", query.language.as_deref().unwrap_or_default());
    add("use_cases", query.use_case.as_deref().unwrap_or_default());
    let reply = http::get(&url, &[("xi-api-key", key)])?;
    if !reply.ok() {
        return Err(error(&reply));
    }
    let json = reply.json()?;
    Ok(VoicePage {
        voices: json["voices"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(shared)
            .collect(),
        more: json["has_more"].as_bool().unwrap_or(false),
    })
}

pub fn similar(key: &str, audio: &Path) -> CloudResult<Vec<Voice>> {
    let reply = http::post_form(
        &format!("{API}/v1/similar-voices"),
        &[("xi-api-key", key)],
        &[("top_k", SIMILAR.to_string())],
        &[("audio_file", audio, http::audio_mime(audio))],
        true,
    )?;
    if !reply.ok() {
        return Err(error(&reply));
    }
    let json = reply.json()?;
    Ok(json["voices"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(shared)
        .collect())
}

pub fn add(key: &str, shared: &Voice, name: &str) -> CloudResult<Voice> {
    let owner = shared
        .owner
        .as_deref()
        .ok_or_else(|| CloudError::Failed(format!("{} is on the account already", shared.name)))?;
    let url = format!(
        "{API}/v1/voices/add/{}/{}",
        path_part(owner)?,
        path_part(&shared.id)?
    );
    let reply = http::post_json(
        &url,
        &[("xi-api-key", key)],
        &json!({ "new_name": name }),
        false,
        None,
    )?;
    if !reply.ok() {
        return Err(error(&reply));
    }
    let id = reply.json()?["voice_id"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| CloudError::Failed("the reply named no voice".into()))?;
    Ok(Voice {
        id,
        name: name.to_owned(),
        owner: None,
        ..shared.clone()
    })
}

pub fn clone(key: &str, clone: &VoiceClone) -> CloudResult<Voice> {
    let mut fields = vec![
        ("name", clone.name.clone()),
        ("remove_background_noise", clone.denoise.to_string()),
    ];
    if !clone.description.trim().is_empty() {
        fields.push(("description", clone.description.trim().to_owned()));
    }
    let files: Vec<http::FormFile> = clone
        .files
        .iter()
        .map(|p| ("files", p.as_path(), http::audio_mime(p)))
        .collect();
    let reply = http::post_form(
        &format!("{API}/v1/voices/add"),
        &[("xi-api-key", key)],
        &fields,
        &files,
        false,
    )?;
    if !reply.ok() {
        return Err(error(&reply));
    }
    let id = reply.json()?["voice_id"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| CloudError::Failed("the reply named no voice".into()))?;
    Ok(Voice {
        id,
        name: clone.name.clone(),
        category: "cloned".into(),
        description: clone.description.clone(),
        ..Voice::default()
    })
}

pub fn rename(key: &str, voice: &str, name: &str) -> CloudResult<()> {
    let url = format!("{API}/v1/voices/{}/edit", path_part(voice)?);
    let reply = http::post_form(
        &url,
        &[("xi-api-key", key)],
        &[("name", name.to_owned())],
        &[],
        false,
    )?;
    if !reply.ok() {
        return Err(error(&reply));
    }
    Ok(())
}

pub fn delete(key: &str, voice: &str) -> CloudResult<()> {
    let url = format!("{API}/v1/voices/{}", path_part(voice)?);
    let reply = http::delete(&url, &[("xi-api-key", key)])?;
    // Already gone is what was asked.
    if !reply.ok() && reply.status != 404 {
        return Err(error(&reply));
    }
    Ok(())
}

/// `id` checked to go in a path: the agent may have written it.
pub(super) fn path_part(id: &str) -> CloudResult<&str> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(CloudError::Failed(format!("No voice with the id “{id}”")));
    }
    Ok(id)
}

/// `text` as a value in a query string.
fn query_value(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b => format!("%{b:02X}"),
        })
        .collect()
}
