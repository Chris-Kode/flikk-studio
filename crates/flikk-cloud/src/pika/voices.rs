//! The account's voices: the stock voices of a speech model, and for
//! Eleven v4 the clones the account made (`/v1/voices`, which takes the key
//! as a bearer token). A clone is made from recordings uploaded to Pika
//! first ([`super::files`]), only with the speaker's consent; Pika can't
//! rename one.

use super::speech::{CLONES, find};
use super::{BASE, NAME, error, files};
use crate::http;
use flikk_core::cloud::{CloudError, CloudResult, GenerationTask, Voice, VoiceClone};
use serde_json::{Value, json};

/// The voices `model` reads with: its stock ones, then the account's
/// clones when it reads them, newest first.
pub fn list(key: &str, model: &str) -> CloudResult<Vec<Voice>> {
    let hosted = find(model)
        .ok_or_else(|| CloudError::Unavailable(format!("{NAME} has no model {model}")))?;
    let mut voices = crate::speech::presets(hosted);
    if model != CLONES {
        return Ok(voices);
    }
    let reply = http::get(
        &format!("{BASE}/v1/voices"),
        &[("authorization", &bearer(key)), ("x-api-key", key)],
    )?;
    if !reply.ok() {
        return Err(error(&reply));
    }
    let json = reply.json()?;
    let clones = json["results"].as_array().into_iter().flatten();
    // Stock voices come first in the list; the model's own are above.
    voices.extend(
        clones
            .filter(|v| v["kind"] == "cloned")
            .filter_map(clone_of),
    );
    Ok(voices)
}

/// Clone a voice from `clone`'s recordings. Can take over a minute; not
/// sent twice, since a clone past the first five is billed.
pub fn clone(key: &str, clone: &VoiceClone) -> CloudResult<Voice> {
    if !clone.consent {
        return Err(CloudError::NotAllowed(
            "Confirm you have the speaker's consent to clone their voice".into(),
        ));
    }
    let name = clone.name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(CloudError::BadPrompt {
            message: "A voice's name runs 1 to 100 characters".into(),
            suggestion: None,
        });
    }
    if clone.files.is_empty() || clone.files.len() > 25 {
        return Err(CloudError::BadPrompt {
            message: format!("{NAME} clones from 1 to 25 recordings"),
            suggestion: None,
        });
    }
    let task = GenerationTask::default();
    let samples = clone
        .files
        .iter()
        .map(|path| files::url(key, path, &task))
        .collect::<CloudResult<Vec<_>>>()?;
    let reply = http::post_json(
        &format!("{BASE}/v1/voices"),
        &[("authorization", &bearer(key)), ("x-api-key", key)],
        &json!({ "name": name, "sample_urls": samples, "consent_attested": true }),
        false,
        None,
    )?;
    if !reply.ok() {
        return Err(match reply.status {
            409 => CloudError::NotAllowed(format!(
                "{NAME} keeps 100 cloned voices at most; delete one first"
            )),
            _ => error(&reply),
        });
    }
    clone_of(&reply.json()?).ok_or_else(|| CloudError::Failed("the reply named no voice".into()))
}

/// Remove clone `voice` from the account; what it read stays.
pub fn delete(key: &str, voice: &str) -> CloudResult<()> {
    if !is_clone(voice) {
        return Err(CloudError::Failed(format!(
            "No voice with the id “{voice}”"
        )));
    }
    let reply = http::delete(
        &format!("{BASE}/v1/voices/{voice}"),
        &[("authorization", &bearer(key)), ("x-api-key", key)],
    )?;
    // Already gone is what was asked.
    if !reply.ok() && reply.status != 404 {
        return Err(error(&reply));
    }
    Ok(())
}

/// Whether `id` is a clone's: `voice_` and 32 hex digits. Checked before it
/// goes in a path, since the agent may have written it.
pub fn is_clone(id: &str) -> bool {
    id.strip_prefix("voice_")
        .is_some_and(|hex| hex.len() == 32 && hex.chars().all(|c| c.is_ascii_hexdigit()))
}

/// One clone, as `/v1/voices` answers it.
fn clone_of(json: &Value) -> Option<Voice> {
    let id = json["voice_id"].as_str().filter(|id| is_clone(id))?;
    Some(Voice {
        id: id.to_owned(),
        name: json["name"].as_str().unwrap_or(id).to_owned(),
        category: "cloned".into(),
        ..Voice::default()
    })
}

/// The key as the voice endpoints take it, a bearer token.
fn bearer(key: &str) -> String {
    format!("Bearer {key}")
}
