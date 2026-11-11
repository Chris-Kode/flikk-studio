//! Music (`POST /v1/music`): from a prompt and a length, or from sections
//! with lengths of their own (a composition plan), so the music changes on
//! cue. The model is always named: the API still defaults to v1.

use super::{FORMATS, cost, write_audio};
use flikk_core::cloud::{CloudError, CloudResult, GenerateRequest, GenerationTask, MusicSection};
use flikk_core::time::Time;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// Best first; 48 kHz MP3 is what the v2 models make.
const OUTPUT: [&str; 3] = ["mp3_48000_320", "mp3_48000_192", "mp3_44100_128"];
/// A piece runs 3 s to 5 min (the API takes 10, the plans sell 5).
const LENGTH_MS: (u64, u64) = (3_000, 300_000);
/// A section runs 3 s to 2 min; a plan has up to 30 of them.
const SECTION_MS: (u64, u64) = (3_000, 120_000);
const SECTIONS: usize = 30;
const PROMPT_CHARS: usize = 4_100;

pub fn run(
    key: &str,
    model: &str,
    request: &GenerateRequest,
    stem: &Path,
    task: &GenerationTask,
) -> CloudResult<(PathBuf, Option<u64>)> {
    let GenerateRequest::Music {
        prompt,
        length,
        instrumental,
        sections,
    } = request
    else {
        unreachable!("music request");
    };
    let mut body = json!({ "model_id": model });
    if sections.is_empty() {
        body["prompt"] = json!(prompt.chars().take(PROMPT_CHARS).collect::<String>());
        body["force_instrumental"] = json!(instrumental);
        if let Some(length) = length {
            body["music_length_ms"] = json!(ms(*length).clamp(LENGTH_MS.0, LENGTH_MS.1));
        }
    } else {
        body["composition_plan"] = plan(model, prompt, *instrumental, sections)?;
        if model == "music_v1" {
            body["respect_sections_durations"] = json!(true);
        }
    }
    let (reply, format) = FORMATS.post("/v1/music", &OUTPUT, key, &body, task)?;
    let path = write_audio(stem, format, &reply.body)?;
    Ok((path, cost(&reply)))
}

fn ms(t: Time) -> u64 {
    (t.as_secs_f64() * 1000.0).round().max(0.0) as u64
}

/// The composition plan for `sections`, in the shape `model` takes: v1
/// has global styles and sections of lines; v2 and later, chunks whose
/// text carries the section's name and lyrics. Also sent to ElevenLabs'
/// models on other hosts.
pub(crate) fn plan(
    model: &str,
    prompt: &str,
    instrumental: bool,
    sections: &[MusicSection],
) -> CloudResult<Value> {
    let refuse = |m: String| {
        Err(CloudError::BadPrompt {
            message: m,
            suggestion: None,
        })
    };
    if sections.len() > SECTIONS {
        return refuse(format!("At most {SECTIONS} sections"));
    }
    let total: u64 = sections.iter().map(|s| ms(s.length)).sum();
    if !(LENGTH_MS.0..=LENGTH_MS.1).contains(&total) {
        return refuse("The sections together run 3 s to 5 min".into());
    }
    if let Some(s) = sections
        .iter()
        .find(|s| !(SECTION_MS.0..=SECTION_MS.1).contains(&ms(s.length)))
    {
        return refuse(format!("Section “{}” must run 3 s to 2 min", s.name));
    }
    let global: Vec<String> = Some(prompt.trim())
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .into_iter()
        .collect();
    let mut avoid_global: Vec<String> = Vec::new();
    if instrumental {
        avoid_global.push("vocals".into());
    }
    let name = |s: &MusicSection| s.name.chars().take(100).collect::<String>();
    if model == "music_v1" {
        return Ok(json!({
            "positive_global_styles": global,
            "negative_global_styles": avoid_global,
            "sections": sections.iter().map(|s| json!({
                "section_name": name(s),
                "positive_local_styles": s.styles,
                "negative_local_styles": s.avoid,
                "duration_ms": ms(s.length),
                "lines": if instrumental { Vec::new() } else { s.lyrics.clone() },
            })).collect::<Vec<_>>(),
        }));
    }
    let chunks: Vec<Value> = sections
        .iter()
        .map(|s| {
            let mut text = format!("[{}]", name(s));
            if instrumental || s.lyrics.is_empty() {
                text.push_str("\n{instrumental}");
            } else {
                for line in s.lyrics.iter().take(30) {
                    text.push('\n');
                    text.extend(line.chars().take(200));
                }
            }
            let styles: Vec<&String> = global.iter().chain(&s.styles).take(50).collect();
            let avoid: Vec<&String> = avoid_global.iter().chain(&s.avoid).take(50).collect();
            json!({
                "text": text,
                "duration_ms": ms(s.length),
                "positive_styles": styles,
                "negative_styles": avoid,
            })
        })
        .collect();
    Ok(json!({ "chunks": chunks }))
}
