//! Voiceovers (`POST /v1/text-to-speech/{voice}`). Eleven v4 reads audio tags
//! and punctuation for delivery and takes only stability and similarity; the
//! API's style and speed don't apply to it, and retiming the clip does what
//! speed would. The model is always named: the API defaults to an older one.
//! A voiceover that continues a recording (a word fixed in a speaker's
//! voice) sends what is said around it as `previous_text` and `next_text`,
//! so its delivery fits; a model that refuses them reads without.

use super::{FORMATS, cost, write_audio};
use flikk_core::cloud::{self, CloudError, CloudResult, GenerateRequest, GenerationTask};
use serde_json::json;
use std::path::{Path, PathBuf};

/// Best first: WAV to keep editing it, which needs the Pro plan; 192 kbps
/// MP3 needs Creator.
const OUTPUT: [&str; 3] = ["wav_48000", "mp3_44100_192", "mp3_44100_128"];

pub fn run(
    key: &str,
    model: &str,
    request: &GenerateRequest,
    stem: &Path,
    task: &GenerationTask,
) -> CloudResult<(PathBuf, Option<u64>)> {
    let GenerateRequest::Voiceover {
        text,
        reading,
        context,
    } = request
    else {
        unreachable!("voiceover request");
    };
    cloud::check_script(text)?;
    // It goes in the path, and the agent may have written it.
    let voice = &reading.voice;
    if voice.is_empty() || !voice.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(CloudError::Failed(format!(
            "No voice with the id “{voice}”"
        )));
    }
    let mut body = json!({
        "text": text,
        "model_id": model,
        "voice_settings": {
            "stability": reading.stability.clamp(0.0, 1.0),
            "similarity_boost": reading.similarity.clamp(0.0, 1.0),
        },
    });
    let endpoint = format!("/v1/text-to-speech/{voice}");
    let around = context
        .as_ref()
        .filter(|c| !c.previous.trim().is_empty() || !c.next.trim().is_empty());
    if let Some(c) = around {
        body["previous_text"] = json!(c.previous.trim());
        body["next_text"] = json!(c.next.trim());
    }
    let (reply, format) = match FORMATS.post(&endpoint, &OUTPUT, key, &body, task) {
        Err(CloudError::Failed(message)) if around.is_some() && names_context(&message) => {
            if let Some(fields) = body.as_object_mut() {
                fields.remove("previous_text");
                fields.remove("next_text");
            }
            FORMATS.post(&endpoint, &OUTPUT, key, &body, task)?
        }
        result => result?,
    };
    let path = write_audio(stem, format, &reply.body)?;
    Ok((path, cost(&reply)))
}

/// Whether a refusal is about the speech around the text, which some
/// models don't take.
fn names_context(message: &str) -> bool {
    message.contains("previous_text") || message.contains("next_text")
}
