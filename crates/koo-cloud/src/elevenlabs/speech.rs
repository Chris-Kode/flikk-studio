//! Voiceovers (`POST /v1/text-to-speech/{voice}`). Eleven v4 reads audio tags
//! and punctuation for delivery and takes only stability and similarity; the
//! API's style and speed don't apply to it, and retiming the clip does what
//! speed would. The model is always named: the API defaults to an older one.

use super::{FORMATS, cost, write_audio};
use koo_core::cloud::{self, CloudError, CloudResult, GenerateRequest};
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
) -> CloudResult<(PathBuf, Option<u64>)> {
    let GenerateRequest::Voiceover { text, reading } = request else {
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
    let body = json!({
        "text": text,
        "model_id": model,
        "voice_settings": {
            "stability": reading.stability.clamp(0.0, 1.0),
            "similarity_boost": reading.similarity.clamp(0.0, 1.0),
        },
    });
    let endpoint = format!("/v1/text-to-speech/{voice}");
    let (reply, format) = FORMATS.post(&endpoint, &OUTPUT, key, &body)?;
    let path = write_audio(stem, format, &reply.body)?;
    Ok((path, cost(&reply)))
}
