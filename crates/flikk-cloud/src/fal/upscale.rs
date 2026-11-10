//! Larger videos: the upscale models fal hosts that the editor offers
//! ([`UPSCALE`]), and how each is asked. The source is uploaded to fal's
//! storage, the model runs on the queue, and the video it made is fetched
//! next to the others; the provider's catalog lists the same ids, in the
//! same order, the default first.

use super::{NAME, files, queue, video};
use crate::http;
use flikk_core::cloud::{
    CloudError, CloudResult, GenerateRequest, GenerationTask, Scale, Stage, UpscaleModel, Upscaling,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// One model: what it can do, and how to ask for it.
pub struct FalUpscale {
    /// The endpoint, kept in the user's settings.
    pub id: &'static str,
    pub spec: UpscaleModel,
    /// Its input for the source at `url`, upscaled as `upscaling` says.
    pub input: fn(url: &str, upscaling: Upscaling) -> Value,
}

const TARGETS: &[&str] = &["720p", "1080p", "2k", "4k"];

pub const UPSCALE: &[FalUpscale] = &[
    FalUpscale {
        id: "topaz/upscale/video/precision",
        spec: UpscaleModel {
            name: "Topaz Precision",
            targets: TARGETS,
            scale: Scale::Factor {
                least: 1.0,
                most: 4.0,
            },
            longest: None,
            largest: None,
            most_pixels: None,
        },
        // Proteus fits most footage, and stays faithful to it.
        input: |url, upscaling| {
            json!({
                "video_url": url,
                "upscale_factor": upscaling.factor,
                "model": "Proteus",
                "H264_output": true,
            })
        },
    },
    FalUpscale {
        id: "topaz/upscale/video/generative",
        spec: UpscaleModel {
            name: "Topaz Starlight",
            targets: TARGETS,
            scale: Scale::Factor {
                least: 1.0,
                most: 4.0,
            },
            longest: None,
            largest: None,
            most_pixels: None,
        },
        input: |url, upscaling| {
            json!({
                "video_url": url,
                "upscale_factor": upscaling.factor,
                "model": "Starlight Precise 2.6",
                "H264_output": true,
            })
        },
    },
    FalUpscale {
        id: "fal-ai/seedvr/upscale/video",
        spec: UpscaleModel {
            name: "SeedVR2",
            targets: TARGETS,
            scale: Scale::Resolution,
            longest: None,
            largest: None,
            most_pixels: None,
        },
        input: |url, upscaling| {
            let target = match upscaling.target {
                "2k" => "1440p",
                "4k" => "2160p",
                other => other,
            };
            json!({
                "video_url": url,
                "upscale_mode": "target",
                "target_resolution": target,
            })
        },
    },
    FalUpscale {
        id: "clarityai/crystal-video-upscaler",
        spec: UpscaleModel {
            name: "Crystal",
            targets: TARGETS,
            scale: Scale::Factor {
                least: 1.0,
                most: 8.0,
            },
            longest: None,
            largest: None,
            // 5K.
            most_pixels: Some(5120 * 2880),
        },
        input: |url, upscaling| json!({ "video_url": url, "scale_factor": upscaling.factor }),
    },
    FalUpscale {
        id: "blackforestlabs/flux-video-upscale",
        spec: UpscaleModel {
            name: "FLUX Video Upscale",
            targets: TARGETS,
            scale: Scale::Factor {
                least: 1.5,
                most: 3.0,
            },
            longest: Some(20),
            largest: None,
            most_pixels: None,
        },
        // Faithful to the source, not inventing detail.
        input: |url, upscaling| {
            json!({
                "video_url": url,
                "upscale_factor": upscaling.factor,
                "creativity": 0,
            })
        },
    },
    FalUpscale {
        id: "google/gemini-omni-flash/v1.2/upscale",
        spec: UpscaleModel {
            name: "Gemini Omni Flash",
            targets: &["720p", "1080p", "4k"],
            scale: Scale::Resolution,
            longest: Some(15),
            largest: None,
            most_pixels: None,
        },
        input: |url, upscaling| json!({ "video_url": url, "resolution": upscaling.target }),
    },
];

pub fn find(id: &str) -> Option<&'static FalUpscale> {
    UPSCALE.iter().find(|m| m.id == id)
}

/// Make `request`'s video larger with `model`, into `<stem>.<ext>`.
pub fn run(
    key: &str,
    model: &FalUpscale,
    request: &GenerateRequest,
    stem: &Path,
    task: &GenerationTask,
) -> CloudResult<(PathBuf, Option<u64>)> {
    let GenerateRequest::Upscale { source, .. } = request else {
        return Err(CloudError::Unavailable(format!(
            "{} only makes videos larger",
            model.spec.name
        )));
    };
    let upscaling = model.spec.plan(request)?;
    let url = files::url(key, &source.path, task)?;
    task.check()?;
    let result = queue::run(key, model.id, &(model.input)(&url, upscaling), task)?;
    let made = &result["video"];
    let url = made["url"]
        .as_str()
        .ok_or_else(|| CloudError::Failed(format!("{NAME} sent no video back")))?;
    let mut name = stem.as_os_str().to_owned();
    name.push(".");
    name.push(video::extension(made));
    let path = PathBuf::from(name);
    task.report(Stage::Downloading(0.0));
    // A public file on fal's CDN: the key doesn't go with it.
    http::download(url, &path, &|f| task.report(Stage::Downloading(f)))?;
    Ok((path, None))
}
