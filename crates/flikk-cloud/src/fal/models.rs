//! The video and image models fal hosts that the editor offers, and how
//! each one takes a request: its endpoints, and the names and forms of its
//! fields, which differ from model to model. What each can make is its
//! [`VideoModel`] or [`ImageModel`]; the provider's catalog lists the same
//! ids, in the same order, the default first.

use flikk_core::cloud::{ImageModel, ReferenceLimits, VideoModel, VideoSound};

/// One model: what it can make, and how to ask for it.
pub struct FalVideo {
    /// The catalog id, kept in the user's settings.
    pub id: &'static str,
    pub spec: VideoModel,
    pub text: Endpoint,
    /// From a first frame, to a last one when the spec says it can.
    pub image: Endpoint,
    /// After references, when the spec takes them.
    pub reference: Option<Endpoint>,
    pub length: Length,
    /// The field the first frame goes in: "image_url", "start_image_url".
    pub start_field: &'static str,
    /// The switch for sound, when the spec makes it.
    pub audio_field: Option<&'static str>,
    /// Lists the references go in, and how the prompt names them.
    pub references: References,
    /// Resolutions are sent in upper case ("768P").
    pub upper_resolution: bool,
    /// Fields sent as they are, every time.
    pub fixed: &'static [(&'static str, &'static str)],
}

/// One endpoint: its id, and what it takes for an aspect ratio.
pub struct Endpoint {
    pub id: &'static str,
    pub aspect: Aspect,
}

/// What an endpoint's `aspect_ratio` takes.
pub enum Aspect {
    /// No field: the model picks (from the first frame, where there is one).
    None,
    /// One of the spec's ratios; left out, the model's default.
    Fixed,
    /// One of the spec's ratios, or this word for the model to pick.
    Auto(&'static str),
}

/// How `duration` is sent.
pub struct Length {
    /// As a string ("5") rather than a number.
    pub text: bool,
    /// What lets the model choose when no length is asked for.
    pub auto: AutoLength,
}

pub enum AutoLength {
    /// Leave the field out: the model's default length.
    Omit,
    /// Send "auto".
    Word,
    /// Send null.
    Null,
}

/// Where references go, and how the prompt names them. The editor names
/// them `@Image1`, `@Video1`, `@Audio1`; they are rewritten to the model's
/// names before sending.
pub struct References {
    pub images: &'static str,
    pub videos: &'static str,
    pub sounds: &'static str,
    pub tags: Tags,
}

#[derive(Clone, Copy)]
pub enum Tags {
    /// `@Image1`, as the editor writes them.
    At,
    /// `Image 1`, `Video 1`, `Audio 1`.
    Spaced,
    /// `character1` for pictures.
    Character,
    /// `<IMAGE_REF_0>`, `<VIDEO_REF_0>`: counted from 0.
    RefToken,
}

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
const S_5_20: [u32; 16] = span(5);

const WIDE: &[&str] = &["16:9", "9:16", "1:1", "4:3", "3:4", "21:9"];

const NO_REFERENCES: References = References {
    images: "",
    videos: "",
    sounds: "",
    tags: Tags::At,
};

const SEEDANCE_REFERENCES: References = References {
    images: "image_urls",
    videos: "video_urls",
    sounds: "audio_urls",
    tags: Tags::At,
};

const AUTO_WORD: Length = Length {
    text: false,
    auto: AutoLength::Word,
};

const NUMBER: Length = Length {
    text: false,
    auto: AutoLength::Omit,
};

const SEEDANCE_2_SPEC: VideoModel = VideoModel {
    name: "Seedance 2.0",
    lengths: &S_4_15,
    aspects: WIDE,
    resolutions: &["720p", "480p"],
    frame_sets_aspect: false,
    last_frame: true,
    references: Some(ReferenceLimits {
        images: 9,
        videos: 3,
        sounds: 3,
        files: 12,
        seconds: 15.0,
    }),
    audio: VideoSound::Switch,
};

pub const VIDEO: &[FalVideo] = &[
    FalVideo {
        id: "bytedance/seedance-2.0",
        spec: SEEDANCE_2_SPEC,
        text: Endpoint {
            id: "bytedance/seedance-2.0/text-to-video",
            aspect: Aspect::Auto("auto"),
        },
        image: Endpoint {
            id: "bytedance/seedance-2.0/image-to-video",
            aspect: Aspect::Auto("auto"),
        },
        reference: Some(Endpoint {
            id: "bytedance/seedance-2.0/reference-to-video",
            aspect: Aspect::Auto("auto"),
        }),
        length: Length {
            text: true,
            auto: AutoLength::Word,
        },
        start_field: "image_url",
        audio_field: Some("generate_audio"),
        references: SEEDANCE_REFERENCES,
        upper_resolution: false,
        fixed: &[],
    },
    FalVideo {
        id: "bytedance/seedance-2.0/fast",
        spec: VideoModel {
            name: "Seedance 2.0 Fast",
            ..SEEDANCE_2_SPEC
        },
        text: Endpoint {
            id: "bytedance/seedance-2.0/fast/text-to-video",
            aspect: Aspect::Auto("auto"),
        },
        image: Endpoint {
            id: "bytedance/seedance-2.0/fast/image-to-video",
            aspect: Aspect::Auto("auto"),
        },
        reference: Some(Endpoint {
            id: "bytedance/seedance-2.0/fast/reference-to-video",
            aspect: Aspect::Auto("auto"),
        }),
        length: Length {
            text: true,
            auto: AutoLength::Word,
        },
        start_field: "image_url",
        audio_field: Some("generate_audio"),
        references: SEEDANCE_REFERENCES,
        upper_resolution: false,
        fixed: &[],
    },
    FalVideo {
        id: "bytedance/seedance-2.5",
        spec: VideoModel {
            name: "Seedance 2.5",
            lengths: &S_4_30,
            aspects: WIDE,
            resolutions: &["720p", "1080p", "480p"],
            frame_sets_aspect: true,
            last_frame: true,
            references: Some(ReferenceLimits {
                images: 30,
                videos: 10,
                sounds: 10,
                files: 50,
                seconds: 30.0,
            }),
            audio: VideoSound::Switch,
        },
        text: Endpoint {
            id: "bytedance/seedance-2.5/text-to-video",
            aspect: Aspect::Auto("auto"),
        },
        image: Endpoint {
            id: "bytedance/seedance-2.5/image-to-video",
            aspect: Aspect::None,
        },
        reference: Some(Endpoint {
            id: "bytedance/seedance-2.5/reference-to-video",
            aspect: Aspect::Auto("auto"),
        }),
        length: Length {
            text: true,
            auto: AutoLength::Word,
        },
        start_field: "image_url",
        audio_field: Some("generate_audio"),
        references: SEEDANCE_REFERENCES,
        upper_resolution: false,
        fixed: &[],
    },
    FalVideo {
        id: "kling/v4",
        spec: VideoModel {
            name: "Kling 4.0",
            lengths: &S_3_30,
            aspects: &["16:9", "9:16", "1:1", "21:9"],
            resolutions: &["720p", "1080p"],
            frame_sets_aspect: true,
            last_frame: true,
            references: None,
            audio: VideoSound::None,
        },
        text: Endpoint {
            id: "kling/v4/text-to-video",
            aspect: Aspect::Fixed,
        },
        image: Endpoint {
            id: "kling/v4/image-to-video",
            aspect: Aspect::None,
        },
        reference: None,
        length: NUMBER,
        start_field: "start_image_url",
        audio_field: None,
        references: NO_REFERENCES,
        upper_resolution: false,
        fixed: &[],
    },
    FalVideo {
        id: "fal-ai/kling-video/o3/pro",
        spec: VideoModel {
            name: "Kling O3 Pro",
            lengths: &S_3_15,
            aspects: &["16:9", "9:16", "1:1"],
            resolutions: &[],
            frame_sets_aspect: true,
            last_frame: true,
            references: Some(ReferenceLimits {
                images: 4,
                videos: 0,
                sounds: 0,
                files: 4,
                seconds: 0.0,
            }),
            audio: VideoSound::Switch,
        },
        text: Endpoint {
            id: "fal-ai/kling-video/o3/pro/text-to-video",
            aspect: Aspect::Fixed,
        },
        image: Endpoint {
            id: "fal-ai/kling-video/o3/pro/image-to-video",
            aspect: Aspect::None,
        },
        reference: Some(Endpoint {
            id: "fal-ai/kling-video/o3/pro/reference-to-video",
            aspect: Aspect::Fixed,
        }),
        length: Length {
            text: true,
            auto: AutoLength::Omit,
        },
        start_field: "image_url",
        audio_field: Some("generate_audio"),
        references: References {
            images: "image_urls",
            videos: "",
            sounds: "",
            tags: Tags::At,
        },
        upper_resolution: false,
        fixed: &[],
    },
    FalVideo {
        id: "minimax/h3-max",
        spec: VideoModel {
            name: "MiniMax H3 Max",
            lengths: &S_1_15,
            aspects: WIDE,
            resolutions: &["768p", "1080p", "480p"],
            frame_sets_aspect: true,
            last_frame: true,
            references: Some(ReferenceLimits {
                images: 9,
                videos: 3,
                sounds: 3,
                files: 12,
                seconds: 15.0,
            }),
            audio: VideoSound::None,
        },
        text: Endpoint {
            id: "minimax/h3-max/text-to-video",
            aspect: Aspect::Fixed,
        },
        image: Endpoint {
            id: "minimax/h3-max/image-to-video",
            aspect: Aspect::None,
        },
        reference: Some(Endpoint {
            id: "minimax/h3-max/reference-to-video",
            aspect: Aspect::Auto("adaptive"),
        }),
        length: NUMBER,
        start_field: "image_url",
        audio_field: None,
        references: References {
            images: "reference_image_urls",
            videos: "reference_video_urls",
            sounds: "reference_audio_urls",
            tags: Tags::Spaced,
        },
        upper_resolution: true,
        fixed: &[("prompt_expansion_mode", "balanced")],
    },
    FalVideo {
        id: "alibaba/wan-3.0",
        spec: VideoModel {
            name: "Wan 3.0",
            lengths: &S_2_30,
            aspects: &["16:9", "9:16", "1:1", "4:3", "3:4"],
            resolutions: &["720p", "1080p", "480p"],
            frame_sets_aspect: false,
            last_frame: true,
            references: None,
            audio: VideoSound::Switch,
        },
        text: Endpoint {
            id: "alibaba/wan-3.0/text-to-video",
            aspect: Aspect::Auto("adaptive"),
        },
        image: Endpoint {
            id: "alibaba/wan-3.0/image-to-video",
            aspect: Aspect::Auto("adaptive"),
        },
        reference: None,
        length: Length {
            text: false,
            auto: AutoLength::Null,
        },
        start_field: "start_image_url",
        audio_field: Some("audio"),
        references: NO_REFERENCES,
        upper_resolution: false,
        fixed: &[],
    },
    FalVideo {
        id: "alibaba/happy-horse/v1.1",
        spec: VideoModel {
            name: "Happy Horse 1.1",
            lengths: &S_3_15,
            aspects: &[
                "16:9", "9:16", "1:1", "4:3", "3:4", "21:9", "9:21", "5:4", "4:5",
            ],
            resolutions: &["720p", "1080p"],
            frame_sets_aspect: true,
            last_frame: false,
            references: Some(ReferenceLimits {
                images: 9,
                videos: 0,
                sounds: 0,
                files: 9,
                seconds: 0.0,
            }),
            audio: VideoSound::None,
        },
        text: Endpoint {
            id: "alibaba/happy-horse/v1.1/text-to-video",
            aspect: Aspect::Fixed,
        },
        image: Endpoint {
            id: "alibaba/happy-horse/v1.1/image-to-video",
            aspect: Aspect::None,
        },
        reference: Some(Endpoint {
            id: "alibaba/happy-horse/v1.1/reference-to-video",
            aspect: Aspect::Fixed,
        }),
        length: NUMBER,
        start_field: "image_url",
        audio_field: None,
        references: References {
            images: "image_urls",
            videos: "",
            sounds: "",
            tags: Tags::Character,
        },
        upper_resolution: false,
        fixed: &[],
    },
    FalVideo {
        id: "google/gemini-omni-flash/v1.1",
        spec: VideoModel {
            name: "Gemini Omni Flash 1.1",
            lengths: &S_3_10,
            aspects: &["16:9", "9:16"],
            resolutions: &["720p", "1080p", "4k", "360p"],
            frame_sets_aspect: false,
            last_frame: true,
            references: Some(ReferenceLimits {
                images: 10,
                videos: 3,
                sounds: 0,
                files: 13,
                seconds: 30.0,
            }),
            audio: VideoSound::None,
        },
        text: Endpoint {
            id: "google/gemini-omni-flash/v1.1/text-to-video",
            aspect: Aspect::Fixed,
        },
        image: Endpoint {
            id: "google/gemini-omni-flash/v1.1/image-to-video",
            aspect: Aspect::Fixed,
        },
        reference: Some(Endpoint {
            id: "google/gemini-omni-flash/v1.1/reference-to-video",
            aspect: Aspect::Fixed,
        }),
        length: NUMBER,
        start_field: "image_url",
        audio_field: None,
        references: References {
            images: "image_urls",
            videos: "reference_video_urls",
            sounds: "",
            tags: Tags::RefToken,
        },
        upper_resolution: false,
        fixed: &[],
    },
    FalVideo {
        id: "blackforestlabs/flux-3-max",
        spec: VideoModel {
            name: "FLUX 3 Max",
            lengths: &S_5_20,
            aspects: &["16:9", "9:16", "1:1", "4:3", "3:4", "21:9", "2:1"],
            resolutions: &["720p", "1080p"],
            frame_sets_aspect: false,
            last_frame: true,
            references: None,
            audio: VideoSound::Switch,
        },
        text: Endpoint {
            id: "blackforestlabs/flux-3-max/text-to-video",
            aspect: Aspect::Auto("auto"),
        },
        image: Endpoint {
            id: "blackforestlabs/flux-3-max/image-to-video",
            aspect: Aspect::Auto("auto"),
        },
        reference: None,
        length: NUMBER,
        start_field: "image_url",
        audio_field: Some("generate_audio"),
        references: NO_REFERENCES,
        upper_resolution: false,
        fixed: &[],
    },
    FalVideo {
        id: "lightricks/ltx-2.5/pro",
        spec: VideoModel {
            name: "LTX 2.5 Pro",
            lengths: &[6, 8, 10],
            aspects: &["16:9", "9:16"],
            resolutions: &["1080p", "720p"],
            frame_sets_aspect: false,
            last_frame: true,
            references: None,
            audio: VideoSound::Switch,
        },
        text: Endpoint {
            id: "lightricks/ltx-2.5/text-to-video/pro",
            aspect: Aspect::Fixed,
        },
        image: Endpoint {
            id: "lightricks/ltx-2.5/image-to-video/pro",
            aspect: Aspect::Auto("auto"),
        },
        reference: None,
        length: AUTO_WORD,
        start_field: "image_url",
        audio_field: Some("generate_audio"),
        references: NO_REFERENCES,
        upper_resolution: false,
        fixed: &[],
    },
];

pub fn find(id: &str) -> Option<&'static FalVideo> {
    VIDEO.iter().find(|m| m.id == id)
}

// ------------------------------------------------------------------ images

/// One image model: what it can make, and how to ask for it.
pub struct FalImage {
    /// The catalog id, kept in the user's settings.
    pub id: &'static str,
    pub spec: ImageModel,
    /// The endpoint making a picture from a description.
    pub text: &'static str,
    /// The endpoint making one from pictures, when the spec takes them.
    pub edit: Option<&'static str>,
    pub shape: Shape,
    /// Resolutions are sent in lower case ("2k").
    pub lower_resolution: bool,
    pub pictures: Pictures,
    /// `background` takes "transparent" or "opaque".
    pub background: bool,
}

/// How the shape is asked for.
pub enum Shape {
    /// `aspect_ratio`, one of the spec's ratios; this word lets the model
    /// pick, else the field is left out.
    AspectRatio(Option<&'static str>),
    /// `image_size`, the name fal gives the ratio ([`size_name`]); this
    /// name lets the model pick, else the field is left out.
    ImageSize(Option<&'static str>),
}

/// Where the pictures it edits or follows go.
pub enum Pictures {
    /// All in `image_urls`.
    List,
    /// The one to edit in `image_url`, the ones it follows in
    /// `reference_image_urls`.
    FirstAndReferences,
}

/// The ratios fal's named sizes come in.
const SIZES: &[&str] = &["1:1", "16:9", "9:16", "4:3", "3:4"];

/// fal's name for the size of `aspect`, one of [`SIZES`].
pub fn size_name(aspect: &str) -> Option<&'static str> {
    Some(match aspect {
        "1:1" => "square_hd",
        "16:9" => "landscape_16_9",
        "9:16" => "portrait_16_9",
        "4:3" => "landscape_4_3",
        "3:4" => "portrait_4_3",
        _ => return None,
    })
}

const NANO_BANANA_ASPECTS: &[&str] = &[
    "16:9", "9:16", "1:1", "4:3", "3:4", "3:2", "2:3", "5:4", "4:5", "21:9",
];

const FLUX_3_ASPECTS: &[&str] = &[
    "16:9", "9:16", "1:1", "4:3", "3:4", "3:2", "2:3", "5:4", "4:5", "21:9", "2:1", "1:2",
];

const GROK_ASPECTS: &[&str] = &[
    "16:9", "9:16", "1:1", "4:3", "3:4", "3:2", "2:3", "21:9", "2:1", "1:2",
];

/// A model sized by fal's named sizes, with no choice of resolution.
const fn sized(
    name: &'static str,
    auto_aspect: bool,
    references: usize,
    transparent: bool,
) -> ImageModel {
    ImageModel {
        name,
        aspects: SIZES,
        auto_aspect,
        resolutions: &[],
        references,
        transparent,
    }
}

const fn nano_banana(name: &'static str) -> ImageModel {
    ImageModel {
        name,
        aspects: NANO_BANANA_ASPECTS,
        auto_aspect: true,
        resolutions: &["1K", "2K", "4K"],
        references: 14,
        transparent: false,
    }
}

pub const IMAGE: &[FalImage] = &[
    FalImage {
        id: "google/nano-banana-2.1",
        spec: nano_banana("Nano Banana 2.1"),
        text: "google/nano-banana-2.1",
        edit: Some("google/nano-banana-2.1/edit"),
        shape: Shape::AspectRatio(Some("auto")),
        lower_resolution: false,
        pictures: Pictures::List,
        background: false,
    },
    FalImage {
        id: "fal-ai/nano-banana-pro",
        spec: nano_banana("Nano Banana Pro"),
        text: "fal-ai/nano-banana-pro",
        edit: Some("fal-ai/nano-banana-pro/edit"),
        shape: Shape::AspectRatio(Some("auto")),
        lower_resolution: false,
        pictures: Pictures::List,
        background: false,
    },
    FalImage {
        id: "openai/gpt-image-2",
        spec: sized("GPT Image 2", true, 16, true),
        text: "openai/gpt-image-2",
        edit: Some("openai/gpt-image-2/edit"),
        shape: Shape::ImageSize(Some("auto")),
        lower_resolution: false,
        pictures: Pictures::List,
        background: true,
    },
    FalImage {
        id: "openai/gpt-image-2.6/flare",
        spec: sized("GPT Image 2.6 Flare", true, 16, true),
        text: "openai/gpt-image-2.6/flare/text-to-image",
        edit: Some("openai/gpt-image-2.6/flare/edit"),
        shape: Shape::ImageSize(Some("auto")),
        lower_resolution: false,
        pictures: Pictures::List,
        background: true,
    },
    FalImage {
        id: "openai/gpt-image-2.6/sunburst",
        spec: sized("GPT Image 2.6 Sunburst", true, 16, true),
        text: "openai/gpt-image-2.6/sunburst/text-to-image",
        edit: Some("openai/gpt-image-2.6/sunburst/edit"),
        shape: Shape::ImageSize(Some("auto")),
        lower_resolution: false,
        pictures: Pictures::List,
        background: true,
    },
    FalImage {
        id: "blackforestlabs/flux-3",
        spec: ImageModel {
            name: "FLUX 3",
            aspects: FLUX_3_ASPECTS,
            auto_aspect: true,
            resolutions: &["1K", "1.5K", "2K", "4K"],
            references: 10,
            transparent: false,
        },
        text: "blackforestlabs/flux-3/text-to-image",
        edit: Some("blackforestlabs/flux-3/edit-image"),
        shape: Shape::AspectRatio(Some("auto")),
        lower_resolution: true,
        pictures: Pictures::List,
        background: false,
    },
    FalImage {
        id: "blackforestlabs/flux-3/klein",
        spec: sized("FLUX 3 Klein", false, 10, false),
        text: "blackforestlabs/flux-3/klein/text-to-image",
        edit: Some("blackforestlabs/flux-3/klein/edit-image"),
        shape: Shape::ImageSize(None),
        lower_resolution: false,
        pictures: Pictures::List,
        background: false,
    },
    FalImage {
        id: "bytedance/seedream/v5.5",
        spec: sized("Seedream 5.5", true, 10, true),
        text: "bytedance/seedream/v5.5/text-to-image",
        edit: Some("bytedance/seedream/v5.5/edit"),
        shape: Shape::ImageSize(Some("auto_2K")),
        lower_resolution: false,
        pictures: Pictures::List,
        background: true,
    },
    FalImage {
        id: "bytedance/seedream/v5/pro",
        spec: sized("Seedream 5.0 Pro", true, 10, false),
        text: "bytedance/seedream/v5/pro/text-to-image",
        edit: Some("bytedance/seedream/v5/pro/edit"),
        shape: Shape::ImageSize(Some("auto_2K")),
        lower_resolution: false,
        pictures: Pictures::List,
        background: false,
    },
    FalImage {
        id: "ideogram/v4.5",
        spec: sized("Ideogram 4.5", false, 5, false),
        text: "ideogram/v4.5",
        edit: Some("ideogram/v4.5/edit"),
        shape: Shape::ImageSize(None),
        lower_resolution: false,
        pictures: Pictures::FirstAndReferences,
        background: false,
    },
    FalImage {
        id: "xai/grok-imagine-image/v2.0",
        spec: ImageModel {
            name: "Grok Imagine Image 2.0",
            aspects: GROK_ASPECTS,
            auto_aspect: true,
            resolutions: &["1K", "1.5K", "2K"],
            references: 5,
            transparent: false,
        },
        text: "xai/grok-imagine-image/v2.0/text-to-image",
        edit: Some("xai/grok-imagine-image/v2.0/edit"),
        shape: Shape::AspectRatio(Some("auto")),
        lower_resolution: true,
        pictures: Pictures::List,
        background: false,
    },
    FalImage {
        id: "alibaba/qwen-image-3",
        spec: sized("Qwen Image 3", false, 3, false),
        text: "alibaba/qwen-image-3/text-to-image",
        edit: Some("alibaba/qwen-image-3/edit"),
        shape: Shape::ImageSize(None),
        lower_resolution: false,
        pictures: Pictures::List,
        background: false,
    },
    FalImage {
        id: "krea/v3",
        spec: sized("Krea 3", true, 8, false),
        text: "krea/v3/text-to-image",
        edit: Some("krea/v3/edit"),
        shape: Shape::ImageSize(Some("auto")),
        lower_resolution: false,
        pictures: Pictures::List,
        background: false,
    },
    FalImage {
        id: "fal-ai/recraft/v4.1/pro",
        spec: sized("Recraft V4.1 Pro", false, 0, false),
        text: "fal-ai/recraft/v4.1/pro/text-to-image",
        edit: None,
        shape: Shape::ImageSize(None),
        lower_resolution: false,
        pictures: Pictures::List,
        background: false,
    },
];

pub fn find_image(id: &str) -> Option<&'static FalImage> {
    IMAGE.iter().find(|m| m.id == id)
}
