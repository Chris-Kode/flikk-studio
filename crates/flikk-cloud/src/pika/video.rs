//! A video from one of [`models`]: its input for the endpoint its request
//! needs, in its family's fields, made as a job, and the file it made
//! fetched next to the others.
//!
//! Most models name references in the prompt as the editor does
//! (`@Image1`, `@Video1`, `@Audio1`, counted by kind in the order sent);
//! for the others they are renamed (`Image 1`).

use super::models::{self, Family, Frames, Hosted, PikaVideo, Switch};
use super::{NAME, files, jobs};
use crate::http;
use flikk_core::cloud::{
    CloudError, CloudResult, GenerateRequest, GenerationTask, Stage, VideoMode, VideoModel,
};
use flikk_core::model::MediaKind;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

pub fn run(
    key: &str,
    model: &PikaVideo,
    request: &GenerateRequest,
    stem: &Path,
    task: &GenerationTask,
) -> CloudResult<(PathBuf, Option<u64>)> {
    let GenerateRequest::Video {
        prompt,
        length,
        aspect,
        first_frame,
        last_frame,
        references,
        ..
    } = request
    else {
        return Err(CloudError::Unavailable(format!("{NAME} only makes video")));
    };
    let refuse = |message: String| {
        Err(CloudError::BadPrompt {
            message,
            suggestion: None,
        })
    };
    let spec = &model.spec;
    let mode = request.video_mode().unwrap_or(VideoMode::Text);
    let resolution = request_resolution(request, spec);
    let secs = length.map(|l| l.as_secs_f64().round() as u32);
    let mut input = Map::new();
    let mut put = |field: &str, value: Value| {
        input.insert(field.into(), value);
    };
    if !prompt.trim().is_empty() {
        put("prompt", json!(prompt));
    }
    if let Some(resolution) = resolution {
        let upper = matches!(&model.family, Family::Hosted(h) if h.upper_resolution);
        let value = if upper {
            resolution.to_ascii_uppercase()
        } else {
            resolution.to_owned()
        };
        put("resolution", json!(value));
    }
    let path = match (&model.family, mode) {
        (Family::H3, _) => {
            if let Some(secs) = secs {
                put("duration", json!(secs));
            }
            // Left out, Pika picks it from the prompt and the frames.
            if let Some(aspect) = aspect {
                put("aspect_ratio", json!(aspect));
            }
            match mode {
                VideoMode::Text => models::H3_TEXT,
                VideoMode::Image => {
                    if let Some(frame) = first_frame {
                        put("image", json!(files::url(key, frame, task)?));
                    }
                    task.check()?;
                    if let Some(frame) = last_frame {
                        put("last_frame_image", json!(files::url(key, frame, task)?));
                    }
                    models::H3_IMAGE
                }
                VideoMode::Reference => {
                    if !references.iter().any(|r| r.kind != MediaKind::Audio) {
                        return refuse(format!(
                            "{} needs a picture or a clip to follow, not only sounds",
                            spec.name
                        ));
                    }
                    let short = references.iter().find(|r| {
                        r.kind != MediaKind::Image
                            && r.length.is_some_and(|l| {
                                l.as_secs_f64() < models::SHORTEST_REFERENCE - 0.05
                            })
                    });
                    if short.is_some() {
                        return refuse(format!(
                            "{} follows clips and sounds of {} s or more",
                            spec.name,
                            models::SHORTEST_REFERENCE
                        ));
                    }
                    // Each kind together, in the order the prompt counts them.
                    let mut list = Vec::new();
                    for (kind, word) in [
                        (MediaKind::Image, "image"),
                        (MediaKind::Video, "video"),
                        (MediaKind::Audio, "audio"),
                    ] {
                        for reference in references.iter().filter(|r| r.kind == kind) {
                            task.check()?;
                            let url = files::url(key, &reference.path, task)?;
                            list.push(json!({ "type": word, "url": url }));
                        }
                    }
                    put("references", Value::Array(list));
                    models::H3_REFERENCE
                }
            }
        }
        (Family::V25, VideoMode::Text) => {
            if let Some(secs) = secs {
                put("duration_s", json!(secs));
            }
            if let Some(aspect) = aspect {
                put("aspect_ratio", json!(aspect));
            }
            models::V25_TEXT
        }
        // From one frame; the picture sets the shape.
        (Family::V25, VideoMode::Image) if last_frame.is_none() => {
            if let Some(secs) = secs {
                put("duration_s", json!(secs));
            }
            if let Some(frame) = first_frame {
                put("image", json!(files::url(key, frame, task)?));
            }
            models::V25_IMAGE
        }
        // From one frame to another: one Pikaframes transition.
        (Family::V25, VideoMode::Image) => {
            if let Some(r) = resolution
                && !models::KEYFRAME_RESOLUTIONS.contains(&r)
            {
                return refuse(format!(
                    "{} makes {} video between two frames, not {r}",
                    spec.name,
                    models::KEYFRAME_RESOLUTIONS.join(" or ")
                ));
            }
            if let Some(secs) = secs {
                put("transition_duration_s", json!(secs));
            }
            let mut images = Vec::new();
            for frame in first_frame.iter().chain(last_frame) {
                task.check()?;
                images.push(json!(files::url(key, frame, task)?));
            }
            put("images", Value::Array(images));
            models::KEYFRAMES
        }
        (Family::V25, VideoMode::Reference) => {
            return refuse(format!("{} doesn't follow references", spec.name));
        }
        (Family::Hosted(hosted), _) => {
            hosted_input(key, hosted, spec, request, &mut put, task)?;
            match mode {
                VideoMode::Text => hosted.text,
                VideoMode::Image => hosted.image,
                VideoMode::Reference => hosted.reference.ok_or_else(|| CloudError::BadPrompt {
                    message: format!("{} doesn't follow references", spec.name),
                    suggestion: None,
                })?,
            }
        }
    };
    let job = jobs::run(key, path, &Value::Object(input), task)?;
    let video = &job["output"]["video"];
    let url = video["url"]
        .as_str()
        .ok_or_else(|| CloudError::Failed(format!("{NAME} sent no video back")))?;
    let mut name = stem.as_os_str().to_owned();
    name.push(".");
    name.push(extension(video));
    let path = PathBuf::from(name);
    task.report(Stage::Downloading(0.0));
    // A download URL of its own: the key doesn't go with it.
    http::download(url, &path, &|f| task.report(Stage::Downloading(f)))?;
    Ok((path, None))
}

/// The fields of another vendor's model, past the prompt and resolution.
fn hosted_input(
    key: &str,
    hosted: &Hosted,
    spec: &VideoModel,
    request: &GenerateRequest,
    put: &mut impl FnMut(&str, Value),
    task: &GenerationTask,
) -> CloudResult<()> {
    let GenerateRequest::Video {
        prompt,
        length,
        aspect,
        first_frame,
        last_frame,
        references,
        audio,
        ..
    } = request
    else {
        return Ok(());
    };
    let name = spec.name;
    let mode = request.video_mode().unwrap_or(VideoMode::Text);
    let resolution = request_resolution(request, spec);
    for (at, longest) in hosted.longest_at {
        if let Some(l) = length
            && Some(*at) == resolution
            && l.as_secs_f64().round() as u32 > *longest
        {
            return Err(CloudError::BadPrompt {
                message: format!("{name} makes at most {longest} s of video at {at}"),
                suggestion: None,
            });
        }
    }
    match (length, hosted.auto_length) {
        (Some(l), _) => put("duration", json!(l.as_secs_f64().round() as u32)),
        (None, Some(word)) => put("duration", json!(word)),
        (None, None) => {}
    }
    // A first frame sets the shape on an endpoint with no field for it.
    if let (Some(field), Some(aspect)) = (hosted.aspect, aspect)
        && !(mode == VideoMode::Image && spec.frame_sets_aspect)
    {
        put(field, json!(aspect));
    }
    match &hosted.audio {
        Some(Switch::Bool(field)) => put(field, json!(audio)),
        Some(Switch::Word(field, on, off)) => put(field, json!(if *audio { on } else { off })),
        None => {}
    }
    match mode {
        VideoMode::Text => {}
        VideoMode::Image => {
            let mut urls = Vec::new();
            for frame in first_frame.iter().chain(last_frame) {
                task.check()?;
                urls.push(files::url(key, frame, task)?);
            }
            match &hosted.frames {
                Frames::Fields(start, end) => {
                    let mut urls = urls.into_iter();
                    if let Some(url) = urls.next() {
                        put(start, json!(url));
                    }
                    if let Some(url) = urls.next().filter(|_| !end.is_empty()) {
                        put(end, json!(url));
                    }
                }
                Frames::List(field) => put(field, json!(urls)),
                Frames::Keyframes => {
                    let frames: Vec<Value> =
                        urls.iter().map(|url| json!({ "image_url": url })).collect();
                    put("keyframes", Value::Array(frames));
                }
            }
        }
        VideoMode::Reference => {
            let short = references.iter().find(|r| {
                r.kind != MediaKind::Image
                    && r.length
                        .is_some_and(|l| l.as_secs_f64() < hosted.shortest_clip - 0.05)
            });
            if short.is_some() {
                return Err(CloudError::BadPrompt {
                    message: format!(
                        "{name} follows clips and sounds of {} s or more",
                        hosted.shortest_clip
                    ),
                    suggestion: None,
                });
            }
            let lists = &hosted.references;
            for (kind, field) in [
                (MediaKind::Image, lists.images),
                (MediaKind::Video, lists.videos),
                (MediaKind::Audio, lists.sounds),
            ] {
                let mut urls = Vec::new();
                for reference in references.iter().filter(|r| r.kind == kind) {
                    task.check()?;
                    urls.push(Value::String(files::url(key, &reference.path, task)?));
                }
                if urls.is_empty() {
                    continue;
                }
                if field.is_empty() {
                    return Err(CloudError::BadPrompt {
                        message: format!("{name} doesn't follow these references"),
                        suggestion: None,
                    });
                }
                put(field, Value::Array(urls));
            }
            if lists.spaced {
                put("prompt", json!(spaced(prompt)));
            }
        }
    }
    Ok(())
}

/// The resolution asked for, as the model spells it, else its default.
fn request_resolution(request: &GenerateRequest, spec: &VideoModel) -> Option<&'static str> {
    let asked = match request {
        GenerateRequest::Video { resolution, .. } => resolution.as_deref(),
        _ => None,
    };
    asked
        .and_then(|r| spec.resolutions.iter().find(|s| s.eq_ignore_ascii_case(r)))
        .or(spec.resolutions.first())
        .copied()
}

/// `prompt` with the editor's reference tags (`@Image1`, `@Video2`,
/// `@Audio1`) written `Image 1`, `Video 2`, `Audio 1`.
fn spaced(prompt: &str) -> String {
    let mut out = String::with_capacity(prompt.len());
    let mut rest = prompt;
    while let Some(at) = rest.find('@') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let tag = ["Image", "Video", "Audio"].into_iter().find_map(|kind| {
            let digits = after
                .strip_prefix(kind)?
                .chars()
                .take_while(char::is_ascii_digit)
                .count();
            (digits > 0).then_some((kind, digits))
        });
        match tag {
            Some((kind, digits)) => {
                let end = kind.len() + digits;
                out.push_str(&format!("{kind} {}", &after[kind.len()..end]));
                rest = &after[end..];
            }
            None => {
                out.push('@');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The extension of the file Pika made, from its type; mp4 when it doesn't
/// say.
pub(super) fn extension(video: &Value) -> &'static str {
    match video["content_type"].as_str() {
        Some("video/webm") => "webm",
        Some("video/quicktime") => "mov",
        _ => "mp4",
    }
}
