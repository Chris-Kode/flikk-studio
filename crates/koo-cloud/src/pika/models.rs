//! The video models on Pika's API that the editor offers: Pika's own, and
//! the other vendors' Pika hosts. How each takes a request (its endpoints,
//! and the names and forms of its fields) differs from model to model; what
//! each can make is its [`VideoModel`], as Pika's catalog
//! (`GET /catalog/apis/{api_id}?expand=inputs`) gives it. The provider's
//! catalog lists the same ids, in the same order, the default first.

use koo_core::cloud::{ReferenceLimits, VideoModel};

/// One model: what it can make, and how to ask for it.
pub struct PikaVideo {
    /// The catalog id, kept in the user's settings: the start of Pika's
    /// `api_id`s for it.
    pub id: &'static str,
    pub spec: VideoModel,
    pub family: Family,
}

/// How a model's requests are written.
// Only in the static table, never moved: the size of a variant is no cost.
#[allow(clippy::large_enum_variant)]
pub enum Family {
    /// Pika H3 Flex: `duration`, `image` and `last_frame_image`, and a
    /// `references` list of `{type, url}`.
    H3,
    /// Pika 2.5: `duration_s` and `image`; a last frame goes to Pikaframes
    /// (Pika 2.5 Keyframe) as `images`.
    V25,
    /// Another vendor's model, as Pika takes it.
    Hosted(Hosted),
}

/// Another vendor's model: its endpoints (paths after `/v1/media/`) and
/// fields.
pub struct Hosted {
    pub text: &'static str,
    pub image: &'static str,
    /// After references, when the spec takes them.
    pub reference: Option<&'static str>,
    /// What `duration` takes to let the model choose ("auto"); left out
    /// when `None`.
    pub auto_length: Option<&'static str>,
    /// The aspect ratio's field ("ratio", "aspect_ratio"); none when it
    /// has no choice. Left out when the model picks, and with a first
    /// frame when the spec says the frame sets the shape.
    pub aspect: Option<&'static str>,
    /// Resolutions are sent in upper case ("768P").
    pub upper_resolution: bool,
    pub frames: Frames,
    /// The switch for sound, when it has one.
    pub audio: Option<Switch>,
    /// Lists the references go in, and how the prompt names them.
    pub references: Lists,
    /// The shortest clip or sound it follows, in seconds.
    pub shortest_clip: f64,
    /// The longest video it makes at a resolution, where that is shorter.
    pub longest_at: &'static [(&'static str, u32)],
}

/// Where the first frame, and the last when the spec takes one, go.
pub enum Frames {
    /// Each in a field of its own: "image_url", "end_image_url".
    Fields(&'static str, &'static str),
    /// Together in a list of URLs: "image_urls".
    List(&'static str),
    /// Together as `keyframes: [{image_url}]`.
    Keyframes,
}

/// A sound switch: its field, and what it takes for on and off.
pub enum Switch {
    Bool(&'static str),
    Word(&'static str, &'static str, &'static str),
}

/// The fields each kind of reference goes in, empty when it takes none of
/// that kind, and how the prompt names them.
pub struct Lists {
    pub images: &'static str,
    pub videos: &'static str,
    pub sounds: &'static str,
    /// `Image 1`, `Video 1`, `Audio 1` rather than the editor's `@Image1`.
    pub spaced: bool,
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

/// `N` whole seconds from `from`.
const fn span<const N: usize>(from: u32) -> [u32; N] {
    let mut out = [0; N];
    let mut i = 0;
    while i < N {
        out[i] = from + i as u32;
        i += 1;
    }
    out
}

const S_1_15: [u32; 15] = span(1);
const S_2_30: [u32; 29] = span(2);
const S_3_10: [u32; 8] = span(3);
const S_3_15: [u32; 13] = span(3);
const S_3_30: [u32; 28] = span(3);
const S_4_15: [u32; 12] = span(4);
const S_4_30: [u32; 27] = span(4);
const S_5_15: [u32; 11] = span(5);
const S_5_20: [u32; 16] = span(5);

const NO_REFERENCES: Lists = Lists {
    images: "",
    videos: "",
    sounds: "",
    spaced: false,
};

const URL_LISTS: Lists = Lists {
    images: "image_urls",
    videos: "video_urls",
    sounds: "audio_urls",
    spaced: false,
};

const SEEDANCE_ASPECTS: &[&str] = &["1:1", "4:3", "3:4", "16:9", "9:16", "21:9"];

const SEEDANCE_2: VideoModel = VideoModel {
    name: "Seedance 2.0",
    lengths: &S_4_15,
    aspects: SEEDANCE_ASPECTS,
    resolutions: &["720p", "480p", "1080p", "4k"],
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
};

/// Seedance 2.0, 2.0 Fast, 2.0 Mini or 2.5, at these endpoints.
const fn seedance_2(text: &'static str, image: &'static str, reference: &'static str) -> Family {
    Family::Hosted(Hosted {
        text,
        image,
        reference: Some(reference),
        // It must be sent.
        auto_length: Some("auto"),
        aspect: Some("ratio"),
        upper_resolution: false,
        frames: Frames::Fields("image_url", "end_image_url"),
        audio: Some(Switch::Bool("generate_audio")),
        references: URL_LISTS,
        shortest_clip: 2.0,
        longest_at: &[],
    })
}

const KLING_ASPECTS: &[&str] = &["16:9", "9:16", "1:1"];

const HAPPY_HORSE: VideoModel = VideoModel {
    name: "Happy Horse 1.1",
    lengths: &S_3_15,
    aspects: &[
        "16:9", "9:16", "1:1", "4:3", "3:4", "4:5", "5:4", "9:21", "21:9",
    ],
    resolutions: &["1080p", "720p"],
    frame_sets_aspect: false,
    last_frame: false,
    references: Some(ReferenceLimits {
        images: 9,
        videos: 0,
        sounds: 0,
        files: 9,
        seconds: 0.0,
    }),
    audio: false,
};

const fn happy_horse(text: &'static str, image: &'static str, reference: &'static str) -> Family {
    Family::Hosted(Hosted {
        text,
        image,
        reference: Some(reference),
        auto_length: None,
        aspect: Some("ratio"),
        upper_resolution: true,
        frames: Frames::Fields("image_url", ""),
        audio: None,
        references: Lists {
            images: "reference_image_urls",
            ..NO_REFERENCES
        },
        shortest_clip: 0.0,
        longest_at: &[],
    })
}

const WAN_3: VideoModel = VideoModel {
    name: "Wan 3.0",
    lengths: &S_2_30,
    aspects: &["16:9", "4:3", "1:1", "3:4", "9:16"],
    resolutions: &["1080p", "720p", "480p"],
    frame_sets_aspect: false,
    last_frame: true,
    // Pika states no limit on how long the clips and sounds run together;
    // this is the longest video it makes.
    references: Some(ReferenceLimits {
        images: 10,
        videos: 5,
        sounds: 5,
        files: 20,
        seconds: 30.0,
    }),
    audio: true,
};

const fn wan_3(text: &'static str, image: &'static str, reference: &'static str) -> Family {
    Family::Hosted(Hosted {
        text,
        image,
        reference: Some(reference),
        auto_length: Some("auto"),
        aspect: Some("ratio"),
        upper_resolution: false,
        frames: Frames::Fields("first_frame_url", "last_frame_url"),
        audio: Some(Switch::Bool("audio")),
        references: Lists {
            images: "reference_image_urls",
            videos: "reference_video_urls",
            sounds: "reference_audio_urls",
            spaced: false,
        },
        shortest_clip: 0.0,
        longest_at: &[],
    })
}

pub const VIDEO: &[PikaVideo] = &[
    PikaVideo {
        id: "pika/pika-h3-flex",
        spec: VideoModel {
            name: "Pika H3 Flex",
            lengths: &S_5_15,
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
    PikaVideo {
        id: "bytedance/seedance-2.0",
        spec: SEEDANCE_2,
        family: seedance_2(
            "bytedance/seedance-2.0/text-to-video",
            "bytedance/seedance-2.0/image-to-video",
            "bytedance/seedance-2.0/reference-to-video",
        ),
    },
    PikaVideo {
        id: "bytedance/seedance-2.0-fast",
        spec: VideoModel {
            name: "Seedance 2.0 Fast",
            resolutions: &["720p", "480p"],
            ..SEEDANCE_2
        },
        family: seedance_2(
            "bytedance/seedance-2.0-fast/text-to-video",
            "bytedance/seedance-2.0-fast/image-to-video",
            "bytedance/seedance-2.0-fast/reference-to-video",
        ),
    },
    PikaVideo {
        id: "bytedance/seedance-2.0-mini",
        spec: VideoModel {
            name: "Seedance 2.0 Mini",
            resolutions: &["720p", "480p"],
            ..SEEDANCE_2
        },
        family: seedance_2(
            "bytedance/seedance-2.0-mini/text-to-video",
            "bytedance/seedance-2.0-mini/image-to-video",
            "bytedance/seedance-2.0-mini/reference-to-video",
        ),
    },
    PikaVideo {
        id: "bytedance/seedance-2.5",
        spec: VideoModel {
            name: "Seedance 2.5",
            lengths: &S_4_30,
            resolutions: &["720p", "480p", "1080p"],
            // No limit on the files together is stated.
            references: Some(ReferenceLimits {
                images: 30,
                videos: 10,
                sounds: 10,
                files: 50,
                seconds: 30.0,
            }),
            ..SEEDANCE_2
        },
        family: seedance_2(
            "bytedance/seedance-2.5/text-to-video",
            "bytedance/seedance-2.5/image-to-video",
            "bytedance/seedance-2.5/reference-to-video",
        ),
    },
    PikaVideo {
        id: "kling/kling-4.0",
        spec: VideoModel {
            name: "Kling 4.0",
            lengths: &S_3_30,
            aspects: &["21:9", "16:9", "9:16", "1:1"],
            resolutions: &["720p", "1080p"],
            frame_sets_aspect: true,
            last_frame: true,
            references: None,
            audio: true,
        },
        family: Family::Hosted(Hosted {
            text: "kling/kling-4.0/text-to-video",
            image: "kling/kling-4.0/image-to-video",
            reference: None,
            auto_length: None,
            aspect: Some("aspect_ratio"),
            upper_resolution: false,
            frames: Frames::Fields("image_url", "last_frame_url"),
            audio: None,
            references: NO_REFERENCES,
            shortest_clip: 0.0,
            longest_at: &[],
        }),
    },
    PikaVideo {
        id: "kling/kling-3.0",
        spec: VideoModel {
            name: "Kling 3.0",
            lengths: &S_3_15,
            aspects: KLING_ASPECTS,
            resolutions: &["720p", "1080p", "4k"],
            frame_sets_aspect: true,
            last_frame: true,
            references: None,
            audio: true,
        },
        family: Family::Hosted(Hosted {
            text: "kling/kling-3.0/text-to-video",
            image: "kling/kling-3.0/image-to-video",
            reference: None,
            auto_length: None,
            aspect: Some("aspect_ratio"),
            upper_resolution: false,
            frames: Frames::Fields("image_url", "last_frame_url"),
            audio: Some(Switch::Word("audio", "native", "off")),
            references: NO_REFERENCES,
            shortest_clip: 0.0,
            longest_at: &[],
        }),
    },
    PikaVideo {
        id: "kling/kling-3.0-turbo",
        spec: VideoModel {
            name: "Kling 3.0 Turbo",
            lengths: &S_3_15,
            aspects: KLING_ASPECTS,
            resolutions: &["720p", "1080p"],
            frame_sets_aspect: true,
            last_frame: false,
            references: None,
            audio: true,
        },
        family: Family::Hosted(Hosted {
            text: "kling/kling-3.0-turbo/text-to-video",
            image: "kling/kling-3.0-turbo/image-to-video",
            reference: None,
            auto_length: None,
            aspect: Some("aspect_ratio"),
            upper_resolution: false,
            frames: Frames::Fields("image_url", ""),
            audio: None,
            references: NO_REFERENCES,
            shortest_clip: 0.0,
            longest_at: &[],
        }),
    },
    PikaVideo {
        id: "minimax/h3",
        spec: VideoModel {
            name: "MiniMax H3",
            lengths: &S_4_15,
            aspects: &["21:9", "16:9", "4:3", "1:1", "3:4", "9:16"],
            resolutions: &["2k", "768p"],
            frame_sets_aspect: true,
            last_frame: true,
            references: Some(ReferenceLimits {
                images: 9,
                videos: 3,
                sounds: 3,
                files: 12,
                seconds: 15.0,
            }),
            audio: false,
        },
        family: Family::Hosted(Hosted {
            text: "minimax/h3/text-to-video",
            image: "minimax/h3/image-to-video",
            reference: Some("minimax/h3/reference-to-video"),
            auto_length: None,
            aspect: Some("ratio"),
            upper_resolution: true,
            frames: Frames::Fields("first_frame_image", "last_frame_image"),
            audio: None,
            references: Lists {
                spaced: true,
                ..URL_LISTS
            },
            shortest_clip: 0.0,
            longest_at: &[],
        }),
    },
    PikaVideo {
        id: "minimax/hailuo-2.3",
        spec: VideoModel {
            name: "Hailuo 2.3",
            lengths: &[6, 10],
            aspects: &[],
            resolutions: &["768p", "1080p"],
            frame_sets_aspect: true,
            last_frame: false,
            references: None,
            audio: false,
        },
        family: Family::Hosted(Hosted {
            text: "minimax/hailuo-2.3/text-to-video",
            image: "minimax/hailuo-2.3/image-to-video",
            reference: None,
            auto_length: None,
            aspect: None,
            upper_resolution: false,
            frames: Frames::Fields("first_frame_image", ""),
            audio: None,
            references: NO_REFERENCES,
            shortest_clip: 0.0,
            // Not in the schema: Pika refuses more at 1080p.
            longest_at: &[("1080p", 6)],
        }),
    },
    PikaVideo {
        id: "alibaba/wan3.0-video",
        spec: WAN_3,
        family: wan_3(
            "alibaba/wan3.0-video/text-to-video",
            "alibaba/wan3.0-video/image-to-video",
            "alibaba/wan3.0-video/omni-video",
        ),
    },
    PikaVideo {
        id: "alibaba/wan3.0-video-prime",
        spec: VideoModel {
            name: "Wan 3.0 Prime",
            ..WAN_3
        },
        family: wan_3(
            "alibaba/wan3.0-video-prime/text-to-video",
            "alibaba/wan3.0-video-prime/image-to-video",
            "alibaba/wan3.0-video-prime/omni-video",
        ),
    },
    PikaVideo {
        id: "alibaba/happyhorse-1.1",
        spec: HAPPY_HORSE,
        family: happy_horse(
            "alibaba/happyhorse-1.1/text-to-video",
            "alibaba/happyhorse-1.1/image-to-video",
            "alibaba/happyhorse-1.1/reference-to-video",
        ),
    },
    PikaVideo {
        id: "alibaba/happyhorse-1.0",
        spec: VideoModel {
            name: "Happy Horse 1.0",
            ..HAPPY_HORSE
        },
        family: happy_horse(
            "alibaba/happyhorse-1.0/text-to-video",
            "alibaba/happyhorse-1.0/image-to-video",
            "alibaba/happyhorse-1.0/reference-to-video",
        ),
    },
    PikaVideo {
        id: "google/gemini-omni-flash",
        spec: VideoModel {
            name: "Gemini Omni Flash",
            lengths: &S_3_10,
            aspects: &["16:9", "9:16"],
            resolutions: &[],
            frame_sets_aspect: false,
            last_frame: true,
            references: Some(ReferenceLimits {
                images: 6,
                videos: 0,
                sounds: 0,
                files: 6,
                seconds: 0.0,
            }),
            audio: false,
        },
        family: Family::Hosted(Hosted {
            text: "google/gemini-omni-flash/text-to-video",
            image: "google/gemini-omni-flash/image-to-video",
            reference: Some("google/gemini-omni-flash/reference-to-video"),
            auto_length: None,
            aspect: Some("aspect_ratio"),
            upper_resolution: false,
            frames: Frames::List("image_urls"),
            audio: None,
            references: Lists {
                images: "image_urls",
                ..NO_REFERENCES
            },
            shortest_clip: 0.0,
            longest_at: &[],
        }),
    },
    PikaVideo {
        id: "black-forest-labs/flux-3-video",
        spec: VideoModel {
            name: "FLUX 3",
            lengths: &S_5_20,
            aspects: &["21:9", "2:1", "16:9", "4:3", "1:1", "3:4", "9:16"],
            resolutions: &["720p", "1080p"],
            frame_sets_aspect: false,
            last_frame: true,
            references: None,
            audio: true,
        },
        family: Family::Hosted(Hosted {
            text: "black-forest-labs/flux-3-video/text-to-video",
            image: "black-forest-labs/flux-3-video/image-to-video",
            reference: None,
            auto_length: Some("auto"),
            aspect: Some("aspect_ratio"),
            upper_resolution: false,
            frames: Frames::Keyframes,
            audio: Some(Switch::Bool("generate_audio")),
            references: NO_REFERENCES,
            shortest_clip: 0.0,
            longest_at: &[],
        }),
    },
    PikaVideo {
        id: "x-ai/grok-imagine-video",
        spec: VideoModel {
            name: "Grok Imagine Video",
            lengths: &S_1_15,
            aspects: &["1:1", "16:9", "9:16", "4:3", "3:4", "3:2", "2:3"],
            resolutions: &["720p", "480p"],
            frame_sets_aspect: false,
            last_frame: false,
            references: Some(ReferenceLimits {
                images: 3,
                videos: 0,
                sounds: 0,
                files: 3,
                seconds: 0.0,
            }),
            audio: false,
        },
        family: Family::Hosted(Hosted {
            text: "x-ai/grok-imagine-video/text-to-video",
            image: "x-ai/grok-imagine-video/image-to-video",
            reference: Some("x-ai/grok-imagine-video/reference-to-video"),
            auto_length: None,
            aspect: Some("aspect_ratio"),
            upper_resolution: false,
            frames: Frames::Fields("image_url", ""),
            audio: None,
            references: Lists {
                images: "reference_image_urls",
                ..NO_REFERENCES
            },
            shortest_clip: 0.0,
            longest_at: &[],
        }),
    },
];

pub fn find(id: &str) -> Option<&'static PikaVideo> {
    VIDEO.iter().find(|m| m.id == id)
}
