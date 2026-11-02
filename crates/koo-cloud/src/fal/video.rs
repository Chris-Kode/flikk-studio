//! A video from a Seedance model: its input for the endpoint its request
//! needs (text, image or reference to video), made on the queue, and the
//! file it made fetched next to the others.

use super::{NAME, files, queue};
use crate::http;
use koo_core::cloud::{
    CloudError, CloudResult, GenerateRequest, GenerationTask, Stage, VideoMode, VideoModel,
};
use koo_core::model::MediaKind;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

pub fn run(
    key: &str,
    model: &str,
    spec: &VideoModel,
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
    let mode = request.video_mode().unwrap_or(VideoMode::Text);
    let mut input = Map::new();
    input.insert("prompt".into(), json!(prompt));
    // Whole seconds, as a string; "auto" lets the model choose.
    let duration = length.map_or_else(
        || "auto".to_owned(),
        |l| (l.as_secs_f64().round() as u32).to_string(),
    );
    input.insert("duration".into(), json!(duration));
    let resolution = resolution
        .as_deref()
        .or(spec.resolutions.first().copied())
        .unwrap_or("720p");
    input.insert("resolution".into(), json!(resolution));
    // Where a first frame sets the shape the field takes nothing else.
    if !(mode == VideoMode::Image && spec.frame_sets_aspect) {
        input.insert(
            "aspect_ratio".into(),
            json!(aspect.as_deref().unwrap_or("auto")),
        );
    }
    if spec.audio {
        input.insert("generate_audio".into(), json!(audio));
    }
    match mode {
        VideoMode::Text => {}
        VideoMode::Image => {
            if let Some(frame) = first_frame {
                input.insert("image_url".into(), json!(files::url(key, frame)?));
            }
            task.check()?;
            if let Some(frame) = last_frame {
                input.insert("end_image_url".into(), json!(files::url(key, frame)?));
            }
        }
        VideoMode::Reference => {
            // Each kind in its own list, in the order the prompt counts them.
            let fields = [
                (MediaKind::Image, "image_urls"),
                (MediaKind::Video, "video_urls"),
                (MediaKind::Audio, "audio_urls"),
            ];
            for (kind, field) in fields {
                let mut urls = Vec::new();
                for reference in references.iter().filter(|r| r.kind == kind) {
                    task.check()?;
                    urls.push(Value::String(files::url(key, &reference.path)?));
                }
                if !urls.is_empty() {
                    input.insert(field.into(), Value::Array(urls));
                }
            }
        }
    }
    let endpoint = format!(
        "{model}/{}",
        match mode {
            VideoMode::Text => "text-to-video",
            VideoMode::Image => "image-to-video",
            VideoMode::Reference => "reference-to-video",
        }
    );
    let result = queue::run(key, &endpoint, &Value::Object(input), task)?;
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
