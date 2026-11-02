//! Pika's own video models that the editor offers, and the endpoints each
//! request goes to. What each can make is its [`VideoModel`], as Pika's
//! catalog (`GET /catalog/apis/{api_id}?expand=inputs`) gives it; the
//! provider's catalog lists the same ids, in the same order, the default
//! first.

use koo_core::cloud::{ReferenceLimits, VideoModel};

/// One model: what it can make, and where to ask for it.
pub struct PikaVideo {
    /// The catalog id, kept in the user's settings: the start of Pika's
    /// `api_id`s for it.
    pub id: &'static str,
    pub spec: VideoModel,
    /// How its requests are written.
    pub family: Family,
}

/// The two ways Pika's models take a request; their fields differ.
pub enum Family {
    /// Pika H3 Flex: `duration`, `image` and `last_frame_image`, and a
    /// `references` list of `{type, url}`.
    H3,
    /// Pika 2.5: `duration_s` and `image`; a last frame goes to Pikaframes
    /// (Pika 2.5 Keyframe) as `images`.
    V25,
}

/// The paths the H3 Flex endpoints are at, after `/v1/media/`.
pub const H3_TEXT: &str = "pika/pika-h3-flex/text-to-video";
pub const H3_IMAGE: &str = "pika/pika-h3-flex/image-to-video";
pub const H3_REFERENCE: &str = "pika/pika-h3-flex/reference-to-video";
/// The paths the Pika 2.5 endpoints are at.
pub const V25_TEXT: &str = "pika/pika-2.5/text-to-video";
pub const V25_IMAGE: &str = "pika/pika-2.5/image-to-video";
pub const KEYFRAMES: &str = "pika/pikaframes/image-to-video";

/// The resolutions Pikaframes makes, of Pika 2.5's.
pub const KEYFRAME_RESOLUTIONS: &[&str] = &["1080p", "720p"];

/// The shortest clip or sound H3 Flex follows, in seconds.
pub const SHORTEST_REFERENCE: f64 = 2.0;

pub const VIDEO: &[PikaVideo] = &[
    PikaVideo {
        id: "pika/pika-h3-flex",
        spec: VideoModel {
            name: "Pika H3 Flex",
            lengths: &[5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
            aspects: &[
                "21:9", "16:9", "3:2", "4:3", "5:4", "1:1", "4:5", "3:4", "2:3", "9:16", "9:21",
            ],
            // 480p and 768p are made; the others are 768p, upscaled.
            resolutions: &["768p", "480p", "1080p", "2k", "4k"],
            frame_sets_aspect: false,
            last_frame: true,
            references: Some(ReferenceLimits {
                images: 9,
                videos: 3,
                sounds: 3,
                files: 12,
                seconds: 15.0,
            }),
            audio: true,
        },
        family: Family::H3,
    },
    PikaVideo {
        id: "pika/pika-2.5",
        spec: VideoModel {
            name: "Pika 2.5",
            lengths: &[5, 10],
            aspects: &["5:2", "16:9", "4:3", "1:1", "4:5", "9:16"],
            resolutions: &["1080p", "720p", "480p"],
            frame_sets_aspect: true,
            last_frame: true,
            references: None,
            audio: false,
        },
        family: Family::V25,
    },
];

pub fn find(id: &str) -> Option<&'static PikaVideo> {
    VIDEO.iter().find(|m| m.id == id)
}
