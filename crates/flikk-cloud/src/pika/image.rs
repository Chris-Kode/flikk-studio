//! A picture from one of [`models::IMAGE`]: its input for the endpoint its
//! request needs (text to image, or image to image), in that model's
//! fields, made as a media job, and the file it made fetched next to the
//! others.

use super::models::{ImageShape, PikaImage};
use super::{NAME, files, jobs};
use crate::picture;
use flikk_core::cloud::{CloudError, CloudResult, GenerateRequest, GenerationTask, ImageMode};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

pub fn run(
    key: &str,
    model: &PikaImage,
    request: &GenerateRequest,
    stem: &Path,
    task: &GenerationTask,
) -> CloudResult<(PathBuf, Option<u64>)> {
    let GenerateRequest::Image {
        prompt,
        aspect,
        resolution,
        references,
        transparent,
    } = request
    else {
        return Err(CloudError::Unavailable(format!(
            "{} only makes pictures",
            model.spec.name
        )));
    };
    let spec = &model.spec;
    let refuse = |message: String| CloudError::BadPrompt {
        message,
        suggestion: None,
    };
    let edit = request.image_mode() == Some(ImageMode::Edit);
    if edit && spec.references == 0 {
        return Err(refuse(format!(
            "{} only makes pictures from a description",
            spec.name
        )));
    }
    let mut input = Map::new();
    input.insert("prompt".into(), json!(picture::tags(prompt)));
    match (&model.shape, aspect) {
        (ImageShape::AspectRatio(_), Some(aspect)) => {
            input.insert("aspect_ratio".into(), json!(aspect));
        }
        (ImageShape::AspectRatio(Some(word)), None) => {
            input.insert("aspect_ratio".into(), json!(word));
        }
        (ImageShape::Size(sizes), Some(aspect)) => {
            let size = sizes
                .iter()
                .find(|(ratio, _)| ratio == aspect)
                .map(|(_, size)| *size)
                .ok_or_else(|| refuse(format!("{} makes no {aspect} pictures", spec.name)))?;
            input.insert("size".into(), json!(size));
        }
        _ => {}
    }
    // The spelling the model takes of the resolution asked for, else its
    // default; none when it has no choice.
    let resolution = resolution
        .as_deref()
        .and_then(|r| spec.resolutions.iter().find(|s| s.eq_ignore_ascii_case(r)))
        .or(spec.resolutions.first());
    if let Some(resolution) = resolution.filter(|_| !model.resolution.is_empty()) {
        input.insert(model.resolution.into(), json!(resolution));
    }
    if model.background {
        let background = if *transparent {
            "transparent"
        } else {
            "opaque"
        };
        input.insert("background".into(), json!(background));
    }
    if edit {
        let mut urls = Vec::new();
        for path in references {
            task.check()?;
            urls.push(Value::String(files::url(key, path, task)?));
        }
        input.insert("image_urls".into(), Value::Array(urls));
    }
    let job = jobs::run(key, &model.path(edit), &Value::Object(input), task)?;
    let image = &job["output"]["images"][0];
    let url = image["url"]
        .as_str()
        .ok_or_else(|| CloudError::Failed(format!("{NAME} sent no picture back")))?;
    let path = picture::fetch(url, image["content_type"].as_str(), stem, task)?;
    Ok((path, None))
}
