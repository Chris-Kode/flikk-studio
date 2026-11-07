//! A picture from one of [`models::IMAGE`]: its input for the endpoint its
//! request needs (from a description, or from pictures), in that model's
//! fields, made on the queue, and the file it made fetched next to the
//! others.

use super::models::{self, FalImage, Pictures, Shape};
use super::{NAME, files, queue};
use crate::picture;
use flikk_core::cloud::{CloudError, CloudResult, GenerateRequest, GenerationTask, ImageMode};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

pub fn run(
    key: &str,
    model: &FalImage,
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
    let mode = request.image_mode().unwrap_or(ImageMode::Text);
    let endpoint = match mode {
        ImageMode::Text => model.text,
        ImageMode::Edit => model.edit.ok_or_else(|| {
            refuse(format!(
                "{} only makes pictures from a description",
                spec.name
            ))
        })?,
    };
    let mut input = Map::new();
    input.insert("prompt".into(), json!(picture::tags(prompt)));
    match (&model.shape, aspect) {
        (Shape::AspectRatio(_), Some(aspect)) => {
            input.insert("aspect_ratio".into(), json!(aspect));
        }
        (Shape::AspectRatio(Some(word)), None) => {
            input.insert("aspect_ratio".into(), json!(word));
        }
        (Shape::ImageSize(_), Some(aspect)) => {
            let size = models::size_name(aspect)
                .ok_or_else(|| refuse(format!("{} makes no {aspect} pictures", spec.name)))?;
            input.insert("image_size".into(), json!(size));
        }
        (Shape::ImageSize(Some(name)), None) => {
            input.insert("image_size".into(), json!(name));
        }
        (Shape::AspectRatio(None) | Shape::ImageSize(None), None) => {}
    }
    // The spelling the model takes of the resolution asked for, else its
    // default; none when it has no choice.
    let resolution = resolution
        .as_deref()
        .and_then(|r| spec.resolutions.iter().find(|s| s.eq_ignore_ascii_case(r)))
        .or(spec.resolutions.first());
    if let Some(resolution) = resolution {
        let value = if model.lower_resolution {
            resolution.to_ascii_lowercase()
        } else {
            (*resolution).to_owned()
        };
        input.insert("resolution".into(), json!(value));
    }
    if model.background {
        let background = if *transparent {
            "transparent"
        } else {
            "opaque"
        };
        input.insert("background".into(), json!(background));
    }
    if mode == ImageMode::Edit {
        let mut urls = Vec::new();
        for path in references {
            task.check()?;
            urls.push(Value::String(files::url(key, path, task)?));
        }
        match model.pictures {
            Pictures::List => {
                input.insert("image_urls".into(), Value::Array(urls));
            }
            Pictures::FirstAndReferences => {
                let mut urls = urls.into_iter();
                if let Some(first) = urls.next() {
                    input.insert("image_url".into(), first);
                }
                let rest: Vec<Value> = urls.collect();
                if !rest.is_empty() {
                    input.insert("reference_image_urls".into(), Value::Array(rest));
                }
            }
        }
    }
    let result = queue::run(key, endpoint, &Value::Object(input), task)?;
    let image = &result["images"][0];
    let url = image["url"]
        .as_str()
        .ok_or_else(|| CloudError::Failed(format!("{NAME} sent no picture back")))?;
    let path = picture::fetch(url, image["content_type"].as_str(), stem, task)?;
    Ok((path, None))
}
