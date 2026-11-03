//! Voice design (`POST /v1/text-to-voice/design`): a few voices made to fit
//! a description, heard before one is kept (`POST /v1/text-to-voice`).

use super::{API, error, voices};
use crate::http;
use base64::Engine as _;
use flikk_core::cloud::{CloudError, CloudResult, Designed, Voice, VoiceDesign, VoicePreview};
use serde_json::json;

/// The design model: v3 follows a description most closely; the voice it
/// makes reads with any speech model.
const MODEL: &str = "eleven_ttv_v3";
/// Samples to hear: every plan has it.
const FORMAT: &str = "mp3_44100_128";

pub fn design(key: &str, design: &VoiceDesign) -> CloudResult<Designed> {
    design.check()?;
    let mut body = json!({
        "voice_description": design.description.trim(),
        "model_id": MODEL,
    });
    match design
        .text
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        Some(text) => body["text"] = json!(text),
        None => body["auto_generate_text"] = json!(true),
    }
    let url = format!("{API}/v1/text-to-voice/design?output_format={FORMAT}");
    let reply = http::post_json(&url, &[("xi-api-key", key)], &body, false)?;
    if !reply.ok() {
        return Err(error(&reply));
    }
    let json = reply.json()?;
    let dir = flikk_core::paths::cache().join("voice-previews");
    std::fs::create_dir_all(&dir)
        .map_err(|e| CloudError::Failed(format!("could not write {}: {e}", dir.display())))?;
    let mut previews = Vec::new();
    for preview in json["previews"].as_array().into_iter().flatten() {
        let (Some(id), Some(audio)) = (
            preview["generated_voice_id"].as_str(),
            preview["audio_base_64"].as_str(),
        ) else {
            continue;
        };
        let name: String = id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(audio)
            .map_err(|e| CloudError::Failed(format!("unexpected reply: {e}")))?;
        let path = dir.join(format!("{name}.mp3"));
        std::fs::write(&path, bytes)
            .map_err(|e| CloudError::Failed(format!("could not write {}: {e}", path.display())))?;
        previews.push(VoicePreview {
            id: id.to_owned(),
            audio: path,
        });
    }
    if previews.is_empty() {
        return Err(CloudError::Failed("ElevenLabs made no voices".into()));
    }
    Ok(Designed {
        previews,
        text: json["text"].as_str().unwrap_or_default().to_owned(),
    })
}

pub fn save(
    key: &str,
    design: &VoiceDesign,
    preview: &VoicePreview,
    name: &str,
) -> CloudResult<Voice> {
    let body = json!({
        "voice_name": name,
        "voice_description": design.description.trim(),
        "generated_voice_id": preview.id,
    });
    let reply = http::post_json(
        &format!("{API}/v1/text-to-voice"),
        &[("xi-api-key", key)],
        &body,
        false,
    )?;
    if !reply.ok() {
        return Err(error(&reply));
    }
    let json = reply.json()?;
    let id = json["voice_id"]
        .as_str()
        .ok_or_else(|| CloudError::Failed("the reply named no voice".into()))?;
    Ok(voices::voice(&json).unwrap_or_else(|| Voice {
        id: id.to_owned(),
        name: name.to_owned(),
        category: "generated".into(),
        description: design.description.clone(),
        ..Voice::default()
    }))
}
