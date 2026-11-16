//! Lip sync: the lip-sync models fal hosts that the editor offers
//! ([`LIPSYNC`]), and how each is asked. The face and the voice are
//! uploaded to fal's storage, the model runs on the queue, and the video it
//! made is fetched next to the others; the provider's catalog lists the
//! same ids, in the same order, the default first. See
//! `docs/providers/falai.md`.

use super::{files, queue, video};
use flikk_core::cloud::{
    CloudError, CloudResult, FaceKind, GenerateRequest, GenerationTask, LipSyncModel,
};
use flikk_core::time::Time;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// One model: what it takes, and how to ask for it.
pub struct FalLipSync {
    /// The endpoint, kept in the user's settings.
    pub id: &'static str,
    pub spec: LipSyncModel,
    /// The most bytes it takes of a face and of a voice, where it says.
    pub largest: (Option<u64>, Option<u64>),
    /// Its input for the face at `face` and the voice at `voice`, which
    /// runs `length` when known.
    pub input: fn(face: &str, voice: &str, length: Option<Time>) -> Value,
}

/// Moves the mouth in a video of any length, to a voice of any length.
const VIDEO: LipSyncModel = LipSyncModel {
    name: "",
    face: FaceKind::Video,
    video_seconds: None,
    voice_seconds: None,
    sides: None,
};

/// Animates a picture to a voice of any length.
const PICTURE: LipSyncModel = LipSyncModel {
    face: FaceKind::Picture,
    ..VIDEO
};

/// Sync's input: the video is cut where the shorter of the two ends.
fn sync(face: &str, voice: &str) -> Value {
    json!({ "video_url": face, "audio_url": voice, "sync_mode": "cut_off" })
}

pub const LIPSYNC: &[FalLipSync] = &[
    FalLipSync {
        id: "fal-ai/sync-lipsync/v2",
        spec: LipSyncModel {
            name: "Sync Lipsync 2",
            ..VIDEO
        },
        largest: (None, None),
        input: |face, voice, _| {
            let mut input = sync(face, voice);
            input["model"] = json!("lipsync-2");
            input
        },
    },
    FalLipSync {
        id: "fal-ai/sync-lipsync/v2/pro",
        spec: LipSyncModel {
            name: "Sync Lipsync 2 Pro",
            ..VIDEO
        },
        largest: (None, None),
        input: |face, voice, _| sync(face, voice),
    },
    FalLipSync {
        id: "fal-ai/sync-lipsync/v3",
        spec: LipSyncModel {
            name: "Sync 3",
            ..VIDEO
        },
        largest: (None, None),
        input: |face, voice, _| sync(face, voice),
    },
    FalLipSync {
        id: "veed/lipsync/v2",
        spec: LipSyncModel {
            name: "VEED Lipsync",
            ..VIDEO
        },
        largest: (None, None),
        input: |face, voice, _| json!({ "video_url": face, "audio_url": voice }),
    },
    FalLipSync {
        id: "fal-ai/heygen/v3/lipsync/precision",
        spec: LipSyncModel {
            name: "HeyGen Lipsync Precision",
            ..VIDEO
        },
        largest: (None, None),
        // Its length stays the face's, so it can stand in for it in clips.
        input: |face, voice, _| {
            json!({
                "video_url": face,
                "audio_url": voice,
                "enable_dynamic_duration": false,
            })
        },
    },
    // An mp4 or mov of 2 to 10 s at 720p or 1080p (sides of 720 to 1920),
    // 100 MB at most; a voice of 2 to 60 s, 5 MB at most.
    FalLipSync {
        id: "fal-ai/kling-video/lipsync/audio-to-video",
        spec: LipSyncModel {
            name: "Kling LipSync",
            face: FaceKind::Video,
            video_seconds: Some((2.0, 10.0)),
            voice_seconds: Some((2.0, 60.0)),
            sides: Some((720, 1920)),
        },
        largest: (Some(100 << 20), Some(5 << 20)),
        input: |face, voice, _| json!({ "video_url": face, "audio_url": voice }),
    },
    FalLipSync {
        id: "fal-ai/latentsync",
        spec: LipSyncModel {
            name: "LatentSync",
            ..VIDEO
        },
        largest: (None, None),
        input: |face, voice, _| json!({ "video_url": face, "audio_url": voice }),
    },
    FalLipSync {
        id: "fal-ai/sync-lipsync/v3/image-to-video",
        spec: LipSyncModel {
            name: "Sync 3 Avatar",
            ..PICTURE
        },
        largest: (None, None),
        input: |face, voice, _| json!({ "image_url": face, "audio_url": voice }),
    },
    FalLipSync {
        id: "fal-ai/kling-video/ai-avatar/v2/pro",
        spec: LipSyncModel {
            name: "Kling AI Avatar",
            ..PICTURE
        },
        largest: (None, None),
        input: |face, voice, _| json!({ "image_url": face, "audio_url": voice }),
    },
    FalLipSync {
        id: "fal-ai/creatify/aurora",
        spec: LipSyncModel {
            name: "Creatify Aurora",
            ..PICTURE
        },
        largest: (None, None),
        input: |face, voice, _| json!({ "image_url": face, "audio_url": voice, "resolution": "720p" }),
    },
    // 1080p takes voices of 30 s at most, 720p of 60 s.
    FalLipSync {
        id: "fal-ai/bytedance/omnihuman/v1.5",
        spec: LipSyncModel {
            name: "OmniHuman 1.5",
            voice_seconds: Some((0.0, 60.0)),
            ..PICTURE
        },
        largest: (None, None),
        input: |face, voice, length| {
            let short = length.is_some_and(|l| l.as_secs_f64() <= 30.0);
            let resolution = if short { "1080p" } else { "720p" };
            json!({ "image_url": face, "audio_url": voice, "resolution": resolution })
        },
    },
];

pub fn find(id: &str) -> Option<&'static FalLipSync> {
    LIPSYNC.iter().find(|m| m.id == id)
}

/// Make `request`'s face say its voice with `model`, into `<stem>.<ext>`.
pub fn run(
    key: &str,
    model: &FalLipSync,
    request: &GenerateRequest,
    stem: &Path,
    task: &GenerationTask,
) -> CloudResult<(PathBuf, Option<u64>)> {
    let GenerateRequest::LipSync { face, voice } = request else {
        return Err(CloudError::Unavailable(format!(
            "{} only lip-syncs",
            model.spec.name
        )));
    };
    model.spec.check(request)?;
    let (largest_face, largest_voice) = model.largest;
    if let Some(largest) = largest_face {
        crate::check_upload(&face.path, &face.name, largest, model.spec.name)?;
    }
    if let Some(largest) = largest_voice {
        crate::check_upload(&voice.path, &voice.name, largest, model.spec.name)?;
    }
    let face_url = files::url(key, &face.path, task)?;
    task.check()?;
    let voice_url = files::url(key, &voice.path, task)?;
    task.check()?;
    let input = (model.input)(&face_url, &voice_url, voice.length);
    let result = queue::run(key, model.id, &input, task)?;
    Ok((video::fetch(&result, stem, task)?, None))
}
