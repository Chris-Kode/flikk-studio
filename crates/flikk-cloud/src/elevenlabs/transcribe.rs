//! Speech to text with Scribe (`POST /v1/speech-to-text`): the whole file
//! in one request, with each word's times and voice, and the audio events
//! ("(laughter)") heard between them. Filler words are kept: cutting dead
//! time needs the "um"s.

use super::{API, error};
use crate::http;
use flikk_core::analysis::Sound;
use flikk_core::cloud::{CloudResult, Heard, HeardWord, SpeechRequest};
use flikk_core::time::{Time, TimeRange};
use serde_json::Value;

pub fn run(
    key: &str,
    model: &str,
    request: &SpeechRequest,
    progress: &(dyn Fn(f32) + Sync),
) -> CloudResult<Heard> {
    let mut fields = vec![
        ("model_id", model.to_owned()),
        ("timestamps_granularity", "word".to_owned()),
        ("tag_audio_events", "true".to_owned()),
        ("diarize", request.speakers.to_string()),
    ];
    if let Some(language) = request.language {
        fields.push(("language_code", language.to_owned()));
    }
    let mime = match request.audio.extension().and_then(|e| e.to_str()) {
        Some("wav") => "audio/wav",
        Some("mp3") => "audio/mpeg",
        _ => "audio/flac",
    };
    // The upload is most of the wait that can be measured; the rest is
    // the provider listening.
    let reply = http::post_file(
        &format!("{API}/v1/speech-to-text"),
        &[("xi-api-key", key)],
        &fields,
        ("file", request.audio, mime),
        &|f| progress(f * 0.9),
        true,
    )?;
    if !reply.ok() {
        return Err(error(&reply));
    }
    let heard = read(model, &reply.json()?);
    progress(1.0);
    Ok(heard)
}

fn read(model: &str, json: &Value) -> Heard {
    let mut voices: Vec<String> = Vec::new();
    let mut words = Vec::new();
    let mut events = Vec::new();
    for w in json["words"].as_array().into_iter().flatten() {
        let (Some(start), Some(end)) = (w["start"].as_f64(), w["end"].as_f64()) else {
            continue;
        };
        let range = TimeRange::new(
            Time::from_secs_f64(start),
            Time::from_secs_f64(end.max(start)),
        );
        let text = w["text"].as_str().unwrap_or_default().trim();
        let confidence = w["logprob"].as_f64().map_or(1.0, f64::exp).clamp(0.0, 1.0) as f32;
        match w["type"].as_str() {
            Some("word") if !text.is_empty() => {
                let speaker = w["speaker_id"].as_str().map(|id| {
                    voices.iter().position(|v| v == id).unwrap_or_else(|| {
                        voices.push(id.to_owned());
                        voices.len() - 1
                    })
                });
                words.push(HeardWord {
                    range,
                    text: text.to_owned(),
                    confidence,
                    speaker,
                });
            }
            Some("audio_event") => {
                let label = text
                    .trim_matches(|c: char| matches!(c, '(' | ')' | '[' | ']'))
                    .trim()
                    .to_lowercase();
                if !label.is_empty() {
                    events.push(Sound {
                        range,
                        label,
                        confidence,
                    });
                }
            }
            _ => {}
        }
    }
    Heard {
        language: json["language_code"].as_str().unwrap_or("und").to_owned(),
        words,
        events,
        engine: format!("elevenlabs/{model}"),
    }
}
