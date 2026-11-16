//! Lip sync: the lip-sync operations on Pika's API that the editor offers
//! ([`LIPSYNC`]), and how each is asked. The face and the voice are
//! uploaded to Pika, the operation runs as a job, and the video it made is
//! fetched next to the others; the provider's catalog lists the same ids,
//! in the same order, the default first. See `docs/providers/pika.md`.

use super::{LARGEST_FILE, LARGEST_VIDEO, NAME, files, jobs, video};
use flikk_core::cloud::{
    CloudError, CloudResult, FaceKind, GenerateRequest, GenerationTask, LipSyncModel,
};
use flikk_core::time::Time;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// One operation: what it takes, and how to ask for it.
pub struct PikaLipSync {
    /// Its endpoint, after `/v1/media/`; kept in the user's settings.
    pub id: &'static str,
    pub spec: LipSyncModel,
    /// Its input for the face at `face` and the voice at `voice`, which
    /// runs `length` when known.
    pub input: fn(face: &str, voice: &str, length: Option<Time>) -> Value,
}

/// Sync on Pika: moves the mouth in a video of any length.
const SYNC: LipSyncModel = LipSyncModel {
    name: "",
    face: FaceKind::Video,
    video_seconds: None,
    voice_seconds: None,
    sides: None,
};

/// Sync's input: the video is cut where the shorter of the two ends (Pika
/// would play it back and forth until the voice ends).
fn sync(face: &str, voice: &str, _: Option<Time>) -> Value {
    json!({ "video_url": face, "audio_url": voice, "sync_mode": "cut_off" })
}

pub const LIPSYNC: &[PikaLipSync] = &[
    PikaLipSync {
        id: "sync/lipsync-2/avatar",
        spec: LipSyncModel {
            name: "Sync Lipsync 2",
            ..SYNC
        },
        input: sync,
    },
    PikaLipSync {
        id: "sync/lipsync-2-pro/avatar",
        spec: LipSyncModel {
            name: "Sync Lipsync 2 Pro",
            ..SYNC
        },
        input: sync,
    },
    PikaLipSync {
        id: "sync/sync-3/avatar",
        spec: LipSyncModel {
            name: "Sync 3",
            ..SYNC
        },
        input: sync,
    },
    // Pika's schema gives no limits; these are Kling's own, as fal's schema
    // for the same model states them. It must be told where the voice
    // ends, so its length must be known.
    PikaLipSync {
        id: "kling/kling-lipsync/avatar",
        spec: LipSyncModel {
            name: "Kling Lipsync",
            face: FaceKind::Video,
            video_seconds: Some((2.0, 10.0)),
            voice_seconds: Some((2.0, 60.0)),
            sides: Some((720, 1920)),
        },
        input: |face, voice, length| {
            json!({
                "video_url": face,
                "audio_url": voice,
                "sound_end_time": length.map_or(0, Time::as_millis),
            })
        },
    },
    PikaLipSync {
        id: "kling/kling-ai-avatar-v2/avatar",
        spec: LipSyncModel {
            name: "Kling AI Avatar",
            face: FaceKind::Picture,
            video_seconds: None,
            voice_seconds: Some((2.0, 300.0)),
            sides: None,
        },
        input: |face, voice, _| json!({ "image_url": face, "sound_file": voice, "mode": "pro" }),
    },
];

pub fn find(id: &str) -> Option<&'static PikaLipSync> {
    LIPSYNC.iter().find(|m| m.id == id)
}

/// Make `request`'s face say its voice with `model`, into `<stem>.<ext>`.
pub fn run(
    key: &str,
    model: &PikaLipSync,
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
    let largest_face = match face.kind {
        FaceKind::Video => LARGEST_VIDEO,
        FaceKind::Picture => LARGEST_FILE,
    };
    crate::check_upload(&face.path, &face.name, largest_face, NAME)?;
    crate::check_upload(&voice.path, &voice.name, LARGEST_FILE, NAME)?;
    let face_url = files::url(key, &face.path, task)?;
    task.check()?;
    let voice_url = files::url(key, &voice.path, task)?;
    task.check()?;
    let input = (model.input)(&face_url, &voice_url, voice.length);
    let job = jobs::run(key, model.id, &input, task)?;
    Ok((video::fetch(&job, stem, task)?, None))
}
