//! Larger videos: the upscale operations on Pika's API that the editor
//! offers ([`UPSCALE`]), and how each is asked. The source is uploaded to
//! Pika, the operation runs as a job, and the video it made is fetched next
//! to the others; the provider's catalog lists the same ids, in the same
//! order, the default first. See `docs/pika.md`.

use super::{NAME, files, jobs, video};
use crate::http;
use flikk_core::cloud::{
    CloudError, CloudResult, GenerateRequest, GenerationTask, Scale, Stage, UpscaleModel, Upscaling,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// One operation: what it can do, and how to ask for it.
pub struct PikaUpscale {
    /// Kept in the user's settings.
    pub id: &'static str,
    pub spec: UpscaleModel,
    /// Its endpoint, after `/v1/media/`.
    pub path: &'static str,
    /// Its input for the source at `url`, upscaled as `upscaling` says.
    pub input: fn(url: &str, upscaling: Upscaling) -> Value,
}

/// The largest video Pika takes as an upload.
const LARGEST_UPLOAD: u64 = 50 << 20;

const TOPAZ: &str = "topaz/topaz-video-upscale/video-upscale";

/// What Topaz on Pika makes: 2, 3 or 4 times larger, up to 4096 × 2160.
const TOPAZ_SPEC: UpscaleModel = UpscaleModel {
    name: "",
    targets: &["720p", "1080p", "2k", "4k"],
    scale: Scale::Steps(&[2, 3, 4]),
    longest: None,
    largest: None,
    most_pixels: Some(4096 * 2160),
};

/// Topaz's input on Pika, with its `model`.
fn topaz(url: &str, upscaling: Upscaling, model: &str) -> Value {
    json!({
        "video_url": url,
        "upscale_factor": format!("x{}", upscaling.factor.round() as u32),
        "model": model,
        "h264_output": true,
    })
}

pub const UPSCALE: &[PikaUpscale] = &[
    PikaUpscale {
        id: "topaz/topaz-video-upscale",
        spec: UpscaleModel {
            name: "Topaz Proteus",
            ..TOPAZ_SPEC
        },
        path: TOPAZ,
        input: |url, upscaling| topaz(url, upscaling, "proteus"),
    },
    PikaUpscale {
        id: "topaz/topaz-video-upscale/starlight-precise-2.6",
        spec: UpscaleModel {
            name: "Topaz Starlight Precise",
            ..TOPAZ_SPEC
        },
        path: TOPAZ,
        input: |url, upscaling| topaz(url, upscaling, "starlight-precise-2.6"),
    },
    PikaUpscale {
        id: "topaz/topaz-video-upscale/starlight-fast-2",
        spec: UpscaleModel {
            name: "Topaz Starlight Fast",
            ..TOPAZ_SPEC
        },
        path: TOPAZ,
        input: |url, upscaling| topaz(url, upscaling, "starlight-fast-2"),
    },
    // Finishes a 480p or 768p Pika H3 Flex video (or any that small),
    // keeping its frames, frame rate and sound.
    PikaUpscale {
        id: "pika/pika-h3-flex/draft-to-video",
        spec: UpscaleModel {
            name: "Pika H3 Flex Upscale",
            targets: &["1080p", "2k", "4k"],
            scale: Scale::Resolution,
            longest: None,
            largest: Some(768),
            most_pixels: None,
        },
        path: "pika/pika-h3-flex/draft-to-video",
        input: |url, upscaling| json!({ "video": url, "resolution": upscaling.target }),
    },
];

pub fn find(id: &str) -> Option<&'static PikaUpscale> {
    UPSCALE.iter().find(|m| m.id == id)
}

/// Make `request`'s video larger with `model`, into `<stem>.<ext>`.
pub fn run(
    key: &str,
    model: &PikaUpscale,
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
    let size = std::fs::metadata(&source.path)
        .map_err(|e| CloudError::Failed(format!("could not read {}: {e}", source.name)))?
        .len();
    if size > LARGEST_UPLOAD {
        return Err(CloudError::BadPrompt {
            message: format!(
                "{NAME} takes videos of {} MB at most; {} is {} MB",
                LARGEST_UPLOAD >> 20,
                source.name,
                size.div_ceil(1 << 20)
            ),
            suggestion: None,
        });
    }
    let url = files::url(key, &source.path, task)?;
    task.check()?;
    let job = jobs::run(key, model.path, &(model.input)(&url, upscaling), task)?;
    let made = &job["output"]["video"];
    let url = made["url"]
        .as_str()
        .ok_or_else(|| CloudError::Failed(format!("{NAME} sent no video back")))?;
    let mut name = stem.as_os_str().to_owned();
    name.push(".");
    name.push(video::extension(made));
    let path = PathBuf::from(name);
    task.report(Stage::Downloading(0.0));
    // A download URL of its own: the key doesn't go with it.
    http::download(url, &path, &|f| task.report(Stage::Downloading(f)))?;
    Ok((path, None))
}
