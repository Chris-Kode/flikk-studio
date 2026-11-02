//! A video from one of [`models`]: its input for the endpoint its request
//! needs (text, image or reference to video), in that model's fields, made
//! on the queue, and the file it made fetched next to the others.

use super::models::{Aspect, AutoLength, Endpoint, FalVideo, Tags};
use super::{NAME, files, queue};
use crate::http;
use koo_core::cloud::{CloudError, CloudResult, GenerateRequest, GenerationTask, Stage, VideoMode};
use koo_core::model::MediaKind;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

pub fn run(
    key: &str,
    model: &FalVideo,
    request: &GenerateRequest,
    stem: &Path,
    task: &GenerationTask,
) -> CloudResult<(PathBuf, Option<u64>)> {
    let GenerateRequest::Video {
        prompt,
        length,
        aspect,
        resolution,
        first_frame,
        last_frame,
        references,
        audio,
    } = request
    else {
        return Err(CloudError::Unavailable(format!("{NAME} only makes video")));
    };
    let spec = &model.spec;
    let mode = request.video_mode().unwrap_or(VideoMode::Text);
    let endpoint: &Endpoint = match mode {
        VideoMode::Text => &model.text,
        VideoMode::Image => &model.image,
        VideoMode::Reference => model.reference.as_ref().ok_or_else(|| {
            CloudError::Unavailable(format!("{} doesn't follow references", spec.name))
        })?,
    };
    let mut input = Map::new();
    let prompt = match mode {
        VideoMode::Reference => retag(prompt, model.references.tags),
        _ => prompt.clone(),
    };
    input.insert("prompt".into(), json!(prompt));
    match (length, &model.length.auto) {
        (Some(l), _) => {
            let secs = l.as_secs_f64().round() as u32;
            let value = if model.length.text {
                json!(secs.to_string())
            } else {
                json!(secs)
            };
            input.insert("duration".into(), value);
        }
        (None, AutoLength::Word) => {
            input.insert("duration".into(), json!("auto"));
        }
        (None, AutoLength::Null) => {
            input.insert("duration".into(), Value::Null);
        }
        (None, AutoLength::Omit) => {}
    }
    // The spelling the model takes of the resolution asked for, else its
    // default; none when it has no choice.
    let resolution = resolution
        .as_deref()
        .and_then(|r| spec.resolutions.iter().find(|s| s.eq_ignore_ascii_case(r)))
        .or(spec.resolutions.first());
    if let Some(resolution) = resolution {
        let value = if model.upper_resolution {
            resolution.to_ascii_uppercase()
        } else {
            (*resolution).to_owned()
        };
        input.insert("resolution".into(), json!(value));
    }
    match (&endpoint.aspect, aspect) {
        (Aspect::None, _) => {}
        (Aspect::Fixed | Aspect::Auto(_), Some(aspect)) => {
            input.insert("aspect_ratio".into(), json!(aspect));
        }
        (Aspect::Auto(word), None) => {
            input.insert("aspect_ratio".into(), json!(word));
        }
        (Aspect::Fixed, None) => {}
    }
    if let Some(field) = model.audio_field {
        input.insert(field.into(), json!(audio));
    }
    for (field, value) in model.fixed {
        input.insert((*field).into(), json!(value));
    }
    match mode {
        VideoMode::Text => {}
        VideoMode::Image => {
            if let Some(frame) = first_frame {
                input.insert(model.start_field.into(), json!(files::url(key, frame)?));
            }
            task.check()?;
            if let Some(frame) = last_frame {
                input.insert("end_image_url".into(), json!(files::url(key, frame)?));
            }
        }
        VideoMode::Reference => {
            // Each kind in its own list, in the order the prompt counts them.
            let lists = &model.references;
            let fields = [
                (MediaKind::Image, lists.images),
                (MediaKind::Video, lists.videos),
                (MediaKind::Audio, lists.sounds),
            ];
            for (kind, field) in fields {
                let mut urls = Vec::new();
                for reference in references.iter().filter(|r| r.kind == kind) {
                    task.check()?;
                    urls.push(Value::String(files::url(key, &reference.path)?));
                }
                if urls.is_empty() {
                    continue;
                }
                if field.is_empty() {
                    return Err(CloudError::BadPrompt {
                        message: format!("{} doesn't follow these references", spec.name),
                        suggestion: None,
                    });
                }
                input.insert(field.into(), Value::Array(urls));
            }
        }
    }
    let result = queue::run(key, endpoint.id, &Value::Object(input), task)?;
    let video = &result["video"];
    let url = video["url"]
        .as_str()
        .ok_or_else(|| CloudError::Failed(format!("{NAME} sent no video back")))?;
    let mut name = stem.as_os_str().to_owned();
    name.push(".");
    name.push(extension(video));
    let path = PathBuf::from(name);
    task.report(Stage::Downloading(0.0));
    // A public file on fal's CDN: the key doesn't go with it.
    http::download(url, &path, &|f| task.report(Stage::Downloading(f)))?;
    Ok((path, None))
}

/// `prompt` with the editor's reference tags (`@Image1`, `@Video2`,
/// `@Audio1`) as the model names them.
fn retag(prompt: &str, tags: Tags) -> String {
    if matches!(tags, Tags::At) {
        return prompt.to_owned();
    }
    let mut out = String::with_capacity(prompt.len());
    let mut rest = prompt;
    while let Some(at) = rest.find('@') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let kind = ["Image", "Video", "Audio"]
            .into_iter()
            .find(|k| after.starts_with(k));
        let digits = kind.map_or(0, |k| {
            after[k.len()..]
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(after.len() - k.len())
        });
        let (Some(kind), true) = (kind, digits > 0) else {
            out.push('@');
            rest = after;
            continue;
        };
        let n: usize = after[kind.len()..kind.len() + digits].parse().unwrap_or(1);
        out.push_str(&match tags {
            Tags::At => format!("@{kind}{n}"),
            Tags::Spaced => format!("{kind} {n}"),
            Tags::Character if kind == "Image" => format!("character{n}"),
            Tags::Character => format!("{kind} {n}"),
            Tags::RefToken => format!(
                "<{}_REF_{}>",
                kind.to_ascii_uppercase(),
                n.saturating_sub(1)
            ),
        });
        rest = &after[kind.len() + digits..];
    }
    out.push_str(rest);
    out
}

/// The extension of the file fal made, from its type or name; mp4 when it
/// says neither.
fn extension(video: &Value) -> &'static str {
    let named = video["file_name"]
        .as_str()
        .and_then(|n| n.rsplit_once('.'))
        .map(|(_, ext)| ext.to_ascii_lowercase());
    match (video["content_type"].as_str(), named.as_deref()) {
        (Some("video/webm"), _) | (_, Some("webm")) => "webm",
        (Some("video/quicktime"), _) | (_, Some("mov")) => "mov",
        _ => "mp4",
    }
}
