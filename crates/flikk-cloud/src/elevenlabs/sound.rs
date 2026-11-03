//! Sound effects (`POST /v1/sound-generation`): 0.5 to 30 s from a
//! description, optionally made to loop.

use super::{FORMATS, cost, write_audio};
use flikk_core::cloud::{CloudResult, GenerateRequest};
use serde_json::json;
use std::path::{Path, PathBuf};

/// Best first: 48 kHz WAV needs the Pro plan, 192 kbps MP3 Creator.
const OUTPUT: [&str; 3] = ["wav_48000", "mp3_44100_192", "mp3_44100_128"];
/// 48 kHz WAV is made for effects that don't loop.
const OUTPUT_LOOPING: [&str; 2] = ["mp3_44100_192", "mp3_44100_128"];
const SECONDS: (f64, f64) = (0.5, 30.0);

pub fn run(
    key: &str,
    model: &str,
    request: &GenerateRequest,
    stem: &Path,
) -> CloudResult<(PathBuf, Option<u64>)> {
    let GenerateRequest::SoundEffect {
        prompt,
        length,
        looping,
    } = request
    else {
        unreachable!("sound effect request");
    };
    let mut body = json!({
        "text": prompt,
        "model_id": model,
        "loop": looping,
    });
    if let Some(length) = length {
        body["duration_seconds"] = json!(length.as_secs_f64().clamp(SECONDS.0, SECONDS.1));
    }
    let formats: &[&'static str] = if *looping { &OUTPUT_LOOPING } else { &OUTPUT };
    let (reply, format) = FORMATS.post("/v1/sound-generation", formats, key, &body)?;
    let path = write_audio(stem, format, &reply.body)?;
    Ok((path, cost(&reply)))
}
