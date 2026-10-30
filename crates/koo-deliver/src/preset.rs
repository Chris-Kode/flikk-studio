//! Render settings and the built-in presets (PRD D1).

use koo_core::model::SequenceSettings;
use koo_core::{Rate, TimeRange};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "codec", rename_all = "snake_case")]
pub enum VideoCodec {
    /// libx264, constant quality.
    H264 { crf: u8 },
    /// libx265, constant quality.
    H265 { crf: u8 },
    /// prores_ks profile 3, 10-bit 4:2:2.
    ProRes422Hq,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "codec", rename_all = "snake_case")]
pub enum AudioCodec {
    Aac {
        kbps: u32,
    },
    /// 24-bit PCM.
    Pcm24,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Container {
    Mp4,
    Mov,
    Wav,
}

impl Container {
    pub fn extension(self) -> &'static str {
        match self {
            Container::Mp4 => "mp4",
            Container::Mov => "mov",
            Container::Wav => "wav",
        }
    }
}

/// Integrated loudness target for the two-pass `loudnorm` (PRD A2).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LoudnessTarget {
    pub lufs: f32,
    pub true_peak_db: f32,
}

impl Default for LoudnessTarget {
    fn default() -> Self {
        Self {
            lufs: -14.0,
            true_peak_db: -1.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RenderSettings {
    pub name: String,
    /// Output size; `None` uses the sequence size.
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Without `width` and `height`: the sequence scaled to this short side,
    /// so "1080p" is 1920 × 1080 for 16:9 and 1080 × 1920 for 9:16.
    #[serde(default)]
    pub short_side: Option<u32>,
    /// Output rate; `None` uses the sequence rate.
    pub rate: Option<Rate>,
    /// `None` renders audio only.
    pub video: Option<VideoCodec>,
    pub audio: AudioCodec,
    pub container: Container,
    pub loudness: Option<LoudnessTarget>,
    pub burn_captions: bool,
    /// Part of the timeline to render; `None` renders all of it.
    pub range: Option<TimeRange>,
}

impl RenderSettings {
    fn video(name: &str, short_side: u32, rate: Rate, crf: u8) -> Self {
        Self {
            name: name.into(),
            width: None,
            height: None,
            short_side: Some(short_side),
            rate: Some(rate),
            video: Some(VideoCodec::H264 { crf }),
            audio: AudioCodec::Aac { kbps: 320 },
            container: Container::Mp4,
            loudness: None,
            burn_captions: true,
            range: None,
        }
    }

    pub fn h264_1080p60() -> Self {
        Self::video("1080p60 H.264", 1080, Rate::FPS_60, 18)
    }

    pub fn h264_1080p30() -> Self {
        Self::video("1080p30 H.264", 1080, Rate::FPS_30, 18)
    }

    pub fn h264_4k30() -> Self {
        Self::video("4K30 H.264", 2160, Rate::FPS_30, 18)
    }

    pub fn prores_422_hq() -> Self {
        Self {
            name: "ProRes 422 HQ".into(),
            width: None,
            height: None,
            short_side: None,
            rate: None,
            video: Some(VideoCodec::ProRes422Hq),
            audio: AudioCodec::Pcm24,
            container: Container::Mov,
            loudness: None,
            burn_captions: true,
            range: None,
        }
    }

    pub fn wav() -> Self {
        Self {
            name: "Audio WAV".into(),
            width: None,
            height: None,
            short_side: None,
            rate: None,
            video: None,
            audio: AudioCodec::Pcm24,
            container: Container::Wav,
            loudness: None,
            burn_captions: false,
            range: None,
        }
    }

    /// Starting point for custom settings: sequence size and rate, H.264.
    pub fn custom() -> Self {
        Self {
            name: "Custom".into(),
            width: None,
            height: None,
            short_side: None,
            rate: None,
            ..Self::h264_1080p60()
        }
    }

    pub fn presets() -> Vec<RenderSettings> {
        vec![
            Self::h264_1080p60(),
            Self::h264_1080p30(),
            Self::h264_4k30(),
            Self::prores_422_hq(),
            Self::wav(),
            Self::custom(),
        ]
    }

    pub fn extension(&self) -> &'static str {
        self.container.extension()
    }

    /// Output frame size for a sequence of `seq`'s size, both sides even.
    pub fn size(&self, seq: &SequenceSettings) -> (u32, u32) {
        match (self.width, self.height, self.short_side) {
            (Some(w), Some(h), _) => (w, h),
            (None, None, Some(short)) => seq.scaled(short),
            (w, h, _) => (w.unwrap_or(seq.width), h.unwrap_or(seq.height)),
        }
    }

    pub(crate) fn codec_args(&self) -> Vec<String> {
        let mut args: Vec<String> = Vec::new();
        let mut push = |a: &[&str]| args.extend(a.iter().map(|s| s.to_string()));
        match self.video {
            Some(VideoCodec::H264 { crf }) => {
                push(&["-c:v", "libx264", "-preset", "medium", "-crf"]);
                push(&[&crf.to_string(), "-pix_fmt", "yuv420p"]);
            }
            Some(VideoCodec::H265 { crf }) => {
                push(&["-c:v", "libx265", "-preset", "medium", "-crf"]);
                push(&[&crf.to_string(), "-pix_fmt", "yuv420p", "-tag:v", "hvc1"]);
            }
            Some(VideoCodec::ProRes422Hq) => {
                push(&["-c:v", "prores_ks", "-profile:v", "3", "-vendor", "apl0"]);
                push(&["-pix_fmt", "yuv422p10le"]);
            }
            None => push(&["-vn"]),
        }
        match self.audio {
            AudioCodec::Aac { kbps } => push(&["-c:a", "aac", "-b:a", &format!("{kbps}k")]),
            AudioCodec::Pcm24 => push(&["-c:a", "pcm_s24le"]),
        }
        if matches!(self.container, Container::Mp4 | Container::Mov) {
            push(&["-movflags", "+faststart"]);
        }
        args
    }
}
