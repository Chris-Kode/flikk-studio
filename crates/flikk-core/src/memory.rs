//! Long-term agent knowledge: style profile and source facts
//! (PRD "Automations and memory").

use crate::ids::*;
use crate::model::{Ease, Fingerprint};
use crate::time::TimeRange;
use serde::{Deserialize, Serialize};

mod caption_patch;

pub use caption_patch::{BackgroundPatch, CaptionStylePatch, MotionPatch, limits};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum RuleScope {
    Global,
    NextVideo,
    Project { project: ProjectId },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleStatus {
    Proposed,
    Approved,
    Rejected,
}

/// PRD S1. Conflicts resolve to the newest approved rule in the narrowest
/// scope (S3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StyleRule {
    pub id: RuleId,
    pub text: String,
    pub scope: RuleScope,
    pub status: RuleStatus,
    pub supersedes: Option<RuleId>,
    /// Unix seconds.
    pub created_at: i64,
    /// Why it was proposed: change set ids, quotes from feedback.
    pub evidence: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptionPosition {
    #[default]
    Bottom,
    Middle,
    Top,
}

/// sRGB with straight alpha, saved as `#RRGGBBAA`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const WHITE: Color = Color::hex(0xFFFFFFFF);
    pub const BLACK: Color = Color::hex(0x000000FF);

    /// From `0xRRGGBBAA`.
    pub const fn hex(rgba: u32) -> Self {
        let [r, g, b, a] = rgba.to_be_bytes();
        Self { r, g, b, a }
    }

    pub fn with_alpha(self, a: u8) -> Self {
        Self { a, ..self }
    }

    /// Accepts `#RGB`, `#RRGGBB` and `#RRGGBBAA`, with or without the `#`.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches('#');
        if !s.is_ascii() {
            return None;
        }
        let byte = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
        match s.len() {
            3 => {
                let n = |i: usize| u8::from_str_radix(&s[i..i + 1], 16).ok().map(|v| v * 17);
                Some(Self {
                    r: n(0)?,
                    g: n(1)?,
                    b: n(2)?,
                    a: 255,
                })
            }
            6 | 8 => Some(Self {
                r: byte(0)?,
                g: byte(2)?,
                b: byte(4)?,
                a: if s.len() == 8 { byte(6)? } else { 255 },
            }),
            _ => None,
        }
    }
}

impl From<Color> for String {
    fn from(c: Color) -> Self {
        format!("#{:02X}{:02X}{:02X}{:02X}", c.r, c.g, c.b, c.a)
    }
}

impl TryFrom<String> for Color {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Color::parse(&s).ok_or_else(|| format!("not a color: {s:?}"))
    }
}

/// A box behind each caption line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptionBackground {
    pub enabled: bool,
    pub color: Color,
    /// Space around the text, at 1080p.
    pub padding: f32,
}

impl Default for CaptionBackground {
    fn default() -> Self {
        Self {
            enabled: false,
            color: Color::BLACK.with_alpha(0xB3),
            padding: 10.0,
        }
    }
}

/// How the spoken word stands out while a caption is on screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptionHighlight {
    #[default]
    None,
    /// The spoken word takes the highlight color.
    Color,
    /// A box in the highlight color behind the spoken word.
    Box,
    /// Words take the highlight color as they are spoken, and keep it.
    Karaoke,
    /// Words appear as they are spoken.
    Reveal,
}

impl CaptionHighlight {
    pub const ALL: [CaptionHighlight; 5] = [
        Self::None,
        Self::Color,
        Self::Box,
        Self::Karaoke,
        Self::Reveal,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Color => "Color",
            Self::Box => "Box",
            Self::Karaoke => "Karaoke",
            Self::Reveal => "Reveal",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MotionKind {
    #[default]
    None,
    Fade,
    /// Grows into place while fading in; shrinks on the way out.
    Pop,
    /// Rises into place while fading in; sinks on the way out.
    Slide,
}

impl MotionKind {
    pub const ALL: [MotionKind; 4] = [Self::None, Self::Fade, Self::Pop, Self::Slide];

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Fade => "Fade",
            Self::Pop => "Pop",
            Self::Slide => "Slide",
        }
    }
}

/// How a caption enters or leaves.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptionMotion {
    pub kind: MotionKind,
    /// Seconds.
    pub duration: f32,
}

impl Default for CaptionMotion {
    fn default() -> Self {
        Self {
            kind: MotionKind::None,
            duration: 0.25,
        }
    }
}

/// Sizes are in pixels at 1080p and scale with the frame's short side.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptionStyle {
    pub name: String,
    pub font: String,
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    pub uppercase: bool,
    pub color: Color,
    /// Extra space after each character.
    pub spacing: f32,
    /// Stroke width.
    pub outline: f32,
    pub outline_color: Color,
    /// Shadow distance.
    pub shadow: f32,
    pub shadow_color: Color,
    /// Degrees clockwise from the right: 45 casts down and right.
    pub shadow_angle: f32,
    pub background: CaptionBackground,
    pub position: CaptionPosition,
    /// Nudge from `position`, in percent of the frame: right and up.
    pub offset_x: f32,
    pub offset_y: f32,
    pub highlight: CaptionHighlight,
    pub highlight_color: Color,
    pub intro: CaptionMotion,
    pub outro: CaptionMotion,
    pub max_chars_per_line: u32,
    pub max_lines: u32,
}

impl Default for CaptionStyle {
    fn default() -> Self {
        Self {
            name: "Default".into(),
            font: "Inter".into(),
            size: 48.0,
            bold: true,
            italic: false,
            uppercase: false,
            color: Color::WHITE,
            spacing: 0.0,
            outline: 2.0,
            outline_color: Color::BLACK,
            shadow: 0.0,
            shadow_color: Color::BLACK.with_alpha(0x80),
            shadow_angle: 45.0,
            background: CaptionBackground::default(),
            position: CaptionPosition::Bottom,
            offset_x: 0.0,
            offset_y: 0.0,
            highlight: CaptionHighlight::None,
            highlight_color: Color::hex(0xFFD60AFF),
            intro: CaptionMotion::default(),
            outro: CaptionMotion::default(),
            max_chars_per_line: 32,
            max_lines: 2,
        }
    }
}

impl CaptionStyle {
    /// This style dressed in `look`: everything but its name, placement and
    /// line rules.
    pub fn with_look(&self, look: &CaptionStyle) -> CaptionStyle {
        CaptionStyle {
            name: self.name.clone(),
            position: self.position,
            offset_x: self.offset_x,
            offset_y: self.offset_y,
            max_chars_per_line: self.max_chars_per_line,
            max_lines: self.max_lines,
            ..look.clone()
        }
    }

    /// Ready-made looks for [`Self::with_look`].
    pub fn looks() -> Vec<CaptionStyle> {
        let base = CaptionStyle::default();
        let look = |name: &str, f: &dyn Fn(&mut CaptionStyle)| {
            let mut s = CaptionStyle {
                name: name.into(),
                ..base.clone()
            };
            f(&mut s);
            s
        };
        let fade = CaptionMotion {
            kind: MotionKind::Fade,
            duration: 0.2,
        };
        let pop = CaptionMotion {
            kind: MotionKind::Pop,
            duration: 0.15,
        };
        vec![
            look("Classic", &|_| {}),
            look("Soft shadow", &|s| {
                s.outline = 0.0;
                s.shadow = 3.0;
                s.shadow_color = Color::BLACK.with_alpha(0xB3);
            }),
            look("Boxed", &|s| {
                s.bold = false;
                s.outline = 0.0;
                s.background.enabled = true;
            }),
            look("Note", &|s| {
                s.color = Color::hex(0x111111FF);
                s.outline = 0.0;
                s.background = CaptionBackground {
                    enabled: true,
                    color: Color::WHITE.with_alpha(0xF2),
                    padding: 12.0,
                };
            }),
            look("Yellow", &|s| {
                s.color = Color::hex(0xFFE14DFF);
                s.outline = 3.0;
            }),
            look("Neon", &|s| {
                s.color = Color::hex(0x7DF9FFFF);
                s.outline = 2.0;
                s.outline_color = Color::hex(0x2A0A5EFF);
                s.shadow = 4.0;
                s.shadow_color = Color::hex(0xFF2BD6CC);
                s.shadow_angle = 135.0;
            }),
            look("Minimal", &|s| {
                s.bold = false;
                s.size = 42.0;
                s.outline = 0.0;
                s.shadow = 2.0;
                s.intro = fade.clone();
                s.outro = fade.clone();
            }),
            look("Spotlight", &|s| {
                s.size = 60.0;
                s.uppercase = true;
                s.outline = 5.0;
                s.highlight = CaptionHighlight::Color;
                s.highlight_color = Color::hex(0x5DFF4AFF);
                s.intro = pop.clone();
            }),
            look("Word box", &|s| {
                s.size = 56.0;
                s.uppercase = true;
                s.outline = 3.0;
                s.highlight = CaptionHighlight::Box;
                s.highlight_color = Color::hex(0x7C3AEDFF);
                s.intro = pop.clone();
            }),
            look("Karaoke", &|s| {
                s.size = 54.0;
                s.outline = 3.0;
                s.highlight = CaptionHighlight::Karaoke;
            }),
            look("Word by word", &|s| {
                s.size = 60.0;
                s.uppercase = true;
                s.outline = 4.0;
                s.shadow = 3.0;
                s.highlight = CaptionHighlight::Reveal;
            }),
            look("Rise", &|s| {
                s.italic = true;
                s.outline = 0.0;
                s.shadow = 3.0;
                s.shadow_color = Color::BLACK.with_alpha(0x99);
                s.intro = CaptionMotion {
                    kind: MotionKind::Slide,
                    duration: 0.3,
                };
                s.outro = fade.clone();
            }),
        ]
    }
}

/// Presets that are part of the profile (PRD S4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Presets {
    pub caption_styles: Vec<CaptionStyle>,
    pub look: Option<std::path::PathBuf>,
    pub ducking_db: f32,
    pub loudness_lufs: f32,
    pub true_peak_db: f32,
    pub zoom_scale: f32,
    pub zoom_ease: Ease,
}

impl Default for Presets {
    fn default() -> Self {
        Self {
            caption_styles: vec![CaptionStyle::default()],
            look: None,
            ducking_db: -12.0,
            loudness_lufs: -14.0,
            true_peak_db: -1.0,
            zoom_scale: 1.8,
            zoom_ease: Ease::EaseInOut,
        }
    }
}

/// One profile per client or channel (PRD S6).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StyleProfile {
    pub id: RuleId,
    pub name: String,
    pub rules: Vec<StyleRule>,
    pub presets: Presets,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactKind {
    SpeakerReference,
    OriginalAudioTrack,
    LiteralQuote,
    /// Read from a collage: names, scores, on-screen text (PRD F5).
    OnScreen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactStatus {
    Proposed,
    Confirmed,
    /// The file behind the fingerprint was replaced (PRD F2).
    Suspended,
}

/// PRD F1: bound to the source fingerprint, never to a path.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceFact {
    pub id: FactId,
    pub fingerprint: Fingerprint,
    pub kind: FactKind,
    pub text: String,
    /// Source-time range the fact is about, if any.
    pub range: Option<TimeRange>,
    pub status: FactStatus,
}
