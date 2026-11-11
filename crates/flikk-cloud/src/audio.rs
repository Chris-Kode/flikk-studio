//! What the music and sound effect clients of hosts share (fal, Pika: they
//! run other vendors' audio models behind one key). Each hosted model is a
//! [`HostedAudio`] saying which of its fields carry what the editor asks
//! for; [`input`] writes a request in those fields, folding what a model
//! has no field for (sections, lyrics, no vocals) into its prompt, and
//! [`fetch`] brings back the file it made.

use crate::http;
use flikk_core::cloud::{
    AudioLengths, AudioModel, CloudError, CloudResult, GenerateRequest, GenerationTask,
    MusicSection, Stage,
};
use flikk_core::time::Time;
use serde_json::{Map, Value, json};
use std::io::Read as _;
use std::path::{Path, PathBuf};

/// One music or sound effect model a host runs, and how it takes a request.
pub struct HostedAudio {
    /// The catalog's id, which is also the endpoint called.
    pub id: &'static str,
    pub spec: AudioModel,
    /// The field the description goes in ("prompt", "text").
    pub prompt: &'static str,
    /// The most characters the prompt field takes.
    pub prompt_chars: usize,
    pub length: Length,
    pub instrumental: Instrumental,
    pub sections: Sections,
    /// The field sung lines go in; `None` puts them in the prompt.
    pub lyrics: Option<&'static str>,
    /// A switch asking it to write the words of a song asked for with
    /// vocals but none given.
    pub write_lyrics: Option<&'static str>,
    /// The field a sound effect's looping goes in, when `spec.looping`.
    pub looping: Option<&'static str>,
    /// Sent with every request: one take instead of several billed ones,
    /// a lossless file for sound effects.
    pub fixed: &'static [(&'static str, Fixed)],
}

/// Lengths from `shortest` to `longest` seconds, `step` apart.
pub const fn lengths(shortest: f64, longest: f64, step: f64) -> Option<AudioLengths> {
    Some(AudioLengths {
        shortest,
        longest,
        step,
    })
}

/// A music model with nothing but a prompt (up to `prompt_chars`): it
/// picks the length, and the parts and vocals are asked for in words.
pub const fn song(id: &'static str, name: &'static str, prompt_chars: usize) -> HostedAudio {
    HostedAudio {
        id,
        spec: AudioModel {
            name,
            lengths: None,
            looping: false,
        },
        prompt: "prompt",
        prompt_chars,
        length: Length::Chosen,
        instrumental: Instrumental::Prompt,
        sections: Sections::Prompt,
        lyrics: None,
        write_lyrics: None,
        looping: None,
        fixed: &[],
    }
}

/// Where a length goes.
pub enum Length {
    /// It picks the length itself.
    Chosen,
    /// Seconds in `field`, whole when the spec's step is; `default` is sent
    /// when none is asked for and the field is required.
    Seconds {
        field: &'static str,
        default: Option<f64>,
    },
    /// Milliseconds in `field`, left out when none is asked for.
    Millis(&'static str),
}

/// How music is kept without vocals.
pub enum Instrumental {
    /// A switch in this field.
    Field(&'static str),
    /// Asked for in the prompt.
    Prompt,
}

/// Where the parts of a piece of music go.
pub enum Sections {
    /// Written into the prompt, with their times.
    Prompt,
    /// An ElevenLabs composition plan (`composition_plan`, v2 chunks).
    ElevenLabsPlan,
    /// Sonilo's `segments`: where each part starts and how it sounds.
    SoniloSegments,
}

/// A value sent with every request.
pub enum Fixed {
    Str(&'static str),
    Int(i64),
}

impl Fixed {
    fn value(&self) -> Value {
        match self {
            Self::Str(s) => json!(s),
            Self::Int(n) => json!(n),
        }
    }
}

/// The input `model` takes for `request`, a music or sound effect request.
pub fn input(model: &HostedAudio, request: &GenerateRequest) -> CloudResult<Value> {
    let mut input = Map::new();
    for (field, value) in model.fixed {
        input.insert((*field).into(), value.value());
    }
    if let (
        Sections::ElevenLabsPlan,
        GenerateRequest::Music {
            prompt,
            instrumental,
            sections,
            ..
        },
    ) = (&model.sections, request)
        && !sections.is_empty()
    {
        // The plan carries the description, the lengths and the vocals;
        // the API takes it instead of a prompt and its fields.
        let plan =
            crate::elevenlabs::composition_plan("music_v2_5", prompt, *instrumental, sections)?;
        input.insert("composition_plan".into(), plan);
        return Ok(Value::Object(input));
    }
    let prompt = match request {
        GenerateRequest::Music {
            prompt,
            instrumental,
            sections,
            ..
        } => music(model, &mut input, prompt, *instrumental, sections)?,
        GenerateRequest::SoundEffect {
            prompt, looping, ..
        } => {
            if let (Some(field), true) = (model.looping, *looping) {
                input.insert(field.into(), json!(true));
            }
            prompt.trim().to_owned()
        }
        _ => {
            return Err(CloudError::Unavailable(format!(
                "{} only makes music or sound effects",
                model.spec.name
            )));
        }
    };
    if !prompt.is_empty() {
        let prompt: String = prompt.chars().take(model.prompt_chars).collect();
        input.insert(model.prompt.into(), json!(prompt));
    }
    if let Some(length) = request.length() {
        put_length(model, &mut input, length);
    } else if let Length::Seconds {
        field,
        default: Some(secs),
    } = model.length
    {
        input.insert(field.into(), seconds(model, secs));
    }
    Ok(Value::Object(input))
}

/// The fields of a music request other than its prompt and length (and
/// other than a composition plan, which [`input`] writes); the prompt,
/// with what the model has no field for written into it.
fn music(
    model: &HostedAudio,
    input: &mut Map<String, Value>,
    prompt: &str,
    instrumental: bool,
    sections: &[MusicSection],
) -> CloudResult<String> {
    let mut text = prompt.trim().to_owned();
    let sung = !instrumental && sections.iter().any(|s| !s.lyrics.is_empty());
    match model.sections {
        Sections::SoniloSegments if !sections.is_empty() => {
            input.insert("segments".into(), segments(sections));
        }
        Sections::Prompt if !sections.is_empty() => {
            push_line(
                &mut text,
                &timed_parts(sections, sung && model.lyrics.is_none()),
            );
        }
        _ => {}
    }
    match (&model.instrumental, instrumental) {
        (Instrumental::Field(field), _) => {
            input.insert((*field).into(), json!(instrumental));
        }
        (Instrumental::Prompt, true) => push_line(&mut text, "Instrumental only, no vocals."),
        (Instrumental::Prompt, false) => {}
    }
    match model.lyrics {
        Some(field) if sung => {
            input.insert(field.into(), json!(lyrics(sections)));
        }
        _ => {
            if let (Some(field), false, false) = (model.write_lyrics, instrumental, sung) {
                input.insert(field.into(), json!(true));
            }
        }
    }
    Ok(text)
}

/// The length in the model's field, on its step.
fn put_length(model: &HostedAudio, input: &mut Map<String, Value>, length: Time) {
    let Some(lengths) = model.spec.lengths else {
        return;
    };
    let secs = lengths.fit(length.as_secs_f64());
    match model.length {
        Length::Chosen => {}
        Length::Seconds { field, .. } => {
            input.insert(field.into(), seconds(model, secs));
        }
        Length::Millis(field) => {
            input.insert(field.into(), json!((secs * 1000.0).round() as u64));
        }
    }
}

/// `secs` as the model's field takes it: whole, or as they are.
fn seconds(model: &HostedAudio, secs: f64) -> Value {
    let whole = model.spec.lengths.is_some_and(|l| l.step >= 1.0);
    if whole {
        json!(secs.round() as u64)
    } else {
        json!(secs)
    }
}

/// `line` after `text`, on a line of its own.
fn push_line(text: &mut String, line: &str) {
    if !text.is_empty() {
        text.push_str("\n\n");
    }
    text.push_str(line);
}

/// "Structure: 0:00–0:12 Intro: soft piano (no drums). …", with what each
/// part sings when `sung`.
fn timed_parts(sections: &[MusicSection], sung: bool) -> String {
    let mut out = String::from("Structure, in this order and at these times:");
    let mut at = Time::ZERO;
    for s in sections {
        let end = at + s.length;
        out.push_str(&format!("\n{}–{} {}", clock(at), clock(end), s.name));
        if !s.styles.is_empty() {
            out.push_str(&format!(": {}", s.styles.join(", ")));
        }
        if !s.avoid.is_empty() {
            out.push_str(&format!(" (no {})", s.avoid.join(", no ")));
        }
        if sung && !s.lyrics.is_empty() {
            out.push_str(&format!(". Sung: \"{}\"", s.lyrics.join(" / ")));
        }
        at = end;
    }
    out
}

/// "1:05".
fn clock(t: Time) -> String {
    let secs = t.as_secs_f64().round() as u64;
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// Sung lines under each part's name in brackets, as song models read
/// them: "[Verse]\nline\nline".
fn lyrics(sections: &[MusicSection]) -> String {
    sections
        .iter()
        .map(|s| {
            let mut part = format!("[{}]", s.name);
            if s.lyrics.is_empty() {
                part.push_str("\n[instrumental]");
            }
            for line in &s.lyrics {
                part.push('\n');
                part.push_str(line);
            }
            part
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The roles Sonilo knows a part by.
const SONILO_LABELS: [&str; 8] = [
    "intro",
    "verse",
    "pre-chorus",
    "chorus",
    "bridge",
    "break",
    "silence",
    "outro",
];

/// Sonilo's segments: where each part starts, what it sounds like (200
/// characters at most), and its role when its name is one Sonilo knows.
fn segments(sections: &[MusicSection]) -> Value {
    let mut at = Time::ZERO;
    let segments: Vec<Value> = sections
        .iter()
        .take(30)
        .map(|s| {
            let mut sound = if s.styles.is_empty() {
                s.name.clone()
            } else {
                s.styles.join(", ")
            };
            if !s.avoid.is_empty() {
                sound.push_str(&format!("; no {}", s.avoid.join(", no ")));
            }
            let mut segment = json!({
                "start": (at.as_secs_f64() * 10.0).round() / 10.0,
                "prompt": sound.chars().take(200).collect::<String>(),
            });
            let name = s.name.trim().to_lowercase();
            if let Some(label) = SONILO_LABELS.iter().find(|l| name.starts_with(*l)) {
                segment["label"] = json!(label);
            }
            at = at + s.length;
            segment
        })
        .collect();
    json!(segments)
}

/// The first file a result names, under `audio`, `audio_file` or `audios`
/// (an object with a `url`, or a list of them): its address and type.
pub fn made(result: &Value) -> Option<(&str, Option<&str>)> {
    ["audio", "audio_file", "audios"]
        .iter()
        .map(|key| &result[key])
        .map(|file| if file.is_array() { &file[0] } else { file })
        .find_map(|file| {
            let url = file["url"].as_str()?;
            Some((url, file["content_type"].as_str()))
        })
}

/// Fetch the sound at `url` to `<stem>.<its extension>`, reporting to
/// `task`. `content_type` is what the host said it is; the file itself
/// has the last word.
pub fn fetch(
    url: &str,
    content_type: Option<&str>,
    stem: &Path,
    task: &GenerationTask,
) -> CloudResult<PathBuf> {
    let path = with_extension(stem, extension(content_type, url));
    task.report(Stage::Downloading(0.0));
    // A file the host hands out on its own: the key doesn't go with it.
    http::download(url, &path, &|f| task.report(Stage::Downloading(f)))?;
    let Some(sniffed) = sniff(&path) else {
        return Ok(path);
    };
    let named = with_extension(stem, sniffed);
    if named != path && crate::settle(&path, &named).is_ok() {
        return Ok(named);
    }
    Ok(path)
}

/// `<stem>.<ext>`, keeping any dots in the stem.
fn with_extension(stem: &Path, ext: &str) -> PathBuf {
    let mut name = stem.as_os_str().to_owned();
    name.push(".");
    name.push(ext);
    PathBuf::from(name)
}

/// The extension of a sound, from its type or its address; mp3 when they
/// say neither.
fn extension(content_type: Option<&str>, url: &str) -> &'static str {
    let named = url
        .split(['?', '#'])
        .next()
        .and_then(|u| u.rsplit_once('.'))
        .map(|(_, ext)| ext.to_ascii_lowercase());
    let kind = content_type.map(|t| t.split(';').next().unwrap_or(t).trim());
    match (kind, named.as_deref()) {
        (Some("audio/wav" | "audio/x-wav" | "audio/wave"), _) | (_, Some("wav")) => "wav",
        (Some("audio/flac" | "audio/x-flac"), _) | (_, Some("flac")) => "flac",
        (Some("audio/mp4" | "audio/m4a" | "audio/x-m4a" | "audio/aac"), _)
        | (_, Some("m4a" | "aac")) => "m4a",
        (Some("audio/ogg" | "audio/opus"), _) | (_, Some("ogg" | "opus")) => "ogg",
        _ => "mp3",
    }
}

/// What the file at `path` is, from its first bytes.
fn sniff(path: &Path) -> Option<&'static str> {
    let mut head = [0u8; 12];
    std::fs::File::open(path).ok()?.read_exact(&mut head).ok()?;
    if head.starts_with(b"RIFF") && &head[8..12] == b"WAVE" {
        Some("wav")
    } else if head.starts_with(b"fLaC") {
        Some("flac")
    } else if head.starts_with(b"OggS") {
        Some("ogg")
    } else if &head[4..8] == b"ftyp" {
        Some("m4a")
    } else if head.starts_with(b"ID3") || (head[0] == 0xFF && head[1] & 0xE0 == 0xE0) {
        Some("mp3")
    } else {
        None
    }
}
