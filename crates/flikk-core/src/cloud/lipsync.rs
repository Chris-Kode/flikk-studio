//! Lip sync: a face in the pool (a video, or a picture) made to say a
//! sound in the pool by a provider, so the mouth follows the voice. What
//! each lip-sync model takes ([`LipSyncModel`]), so a request is checked
//! before anything is uploaded or paid for.

use super::{CloudError, CloudResult, GenerateRequest, pool_name};
use crate::model::{MediaItem, MediaKind};
use crate::time::Time;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// What a face is: a clip to re-mouth, or a still to bring to life.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FaceKind {
    Video,
    Picture,
}

/// The video or picture whose mouth a lip sync moves.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LipSyncFace {
    pub path: PathBuf,
    /// What the pool calls it.
    pub name: String,
    pub kind: FaceKind,
    /// Its picture, in pixels; 0 when not known.
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    /// How long a video runs, when known; `None` for a picture.
    #[serde(default)]
    pub length: Option<Time>,
}

impl LipSyncFace {
    /// `item` as a face; `None` unless it is a video or a picture.
    pub fn of(item: &MediaItem) -> Option<Self> {
        let kind = match item.kind {
            MediaKind::Video => FaceKind::Video,
            MediaKind::Image => FaceKind::Picture,
            MediaKind::Audio => return None,
        };
        let info = item.info.as_ref();
        let picture = info.and_then(|i| i.video.as_ref());
        let length = info
            .map(|i| i.duration)
            .filter(|d| kind == FaceKind::Video && *d > Time::ZERO);
        Some(Self {
            path: item.path.clone(),
            name: pool_name(item),
            kind,
            width: picture.map_or(0, |v| v.width),
            height: picture.map_or(0, |v| v.height),
            length,
        })
    }
}

/// The sound a lip-synced mouth says.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LipSyncVoice {
    pub path: PathBuf,
    /// What the pool calls it.
    pub name: String,
    /// How long it runs, when known.
    #[serde(default)]
    pub length: Option<Time>,
}

impl LipSyncVoice {
    /// `item` as a voice; `None` unless it is a sound.
    pub fn of(item: &MediaItem) -> Option<Self> {
        if item.kind != MediaKind::Audio {
            return None;
        }
        Some(Self {
            path: item.path.clone(),
            name: pool_name(item),
            length: (item.info.as_ref())
                .map(|i| i.duration)
                .filter(|d| *d > Time::ZERO),
        })
    }
}

/// What one lip-sync model takes, so a request is checked before it costs
/// anything. Lengths are `(shortest, longest)` in seconds; a model with a
/// range for a length needs that length known.
#[derive(Clone, Debug, PartialEq)]
pub struct LipSyncModel {
    pub name: &'static str,
    /// The faces it moves: the mouth in a video, or a picture it animates.
    pub face: FaceKind,
    /// How long a video face may run; `None` when any length does.
    pub video_seconds: Option<(f64, f64)>,
    /// How long the voice may run; `None` when any length does.
    pub voice_seconds: Option<(f64, f64)>,
    /// The shortest and longest each side of a video face may be, in
    /// pixels; `None` when any size does. Checked when the size is known.
    pub sides: Option<(u32, u32)>,
}

impl LipSyncModel {
    /// Whether it moves a face of `kind`: the menu offers it only then.
    pub fn takes(&self, kind: FaceKind) -> bool {
        self.face == kind
    }

    /// Why it can't make `request`, before anything is uploaded or spent.
    pub fn check(&self, request: &GenerateRequest) -> CloudResult<()> {
        let GenerateRequest::LipSync { face, voice } = request else {
            return Ok(());
        };
        let name = self.name;
        if !self.takes(face.kind) {
            return refuse(match self.face {
                FaceKind::Video => format!("{name} moves the mouth in a video, not a picture"),
                FaceKind::Picture => format!("{name} animates a picture, not a video"),
            });
        }
        if let Some(range) = self.video_seconds {
            within(
                range,
                face.length,
                &face.name,
                &format!("{name} takes videos"),
            )?;
        }
        if let Some(range) = self.voice_seconds {
            within(
                range,
                voice.length,
                &voice.name,
                &format!("{name} takes voices"),
            )?;
        }
        if let Some((least, most)) = self.sides
            && face.kind == FaceKind::Video
            && face.width > 0
            && face.height > 0
        {
            let (short, long) = (face.width.min(face.height), face.width.max(face.height));
            if short < least || long > most {
                return refuse(format!(
                    "{name} takes videos {least} to {most} pixels a side; {} is {} × {}",
                    face.name, face.width, face.height
                ));
            }
        }
        Ok(())
    }
}

/// `length` in `(shortest, longest)` seconds, or why not, as `what`
/// ("Kling takes voices") says.
fn within(
    (shortest, longest): (f64, f64),
    length: Option<Time>,
    name: &str,
    what: &str,
) -> CloudResult<()> {
    let Some(length) = length else {
        return refuse(format!("The length of {name} isn't known"));
    };
    let secs = length.as_secs_f64();
    // A length read off a file can land a hair outside.
    if secs < shortest - 0.05 || secs > longest + 0.05 {
        let range = if shortest > 0.0 {
            format!("of {} to {} s", seconds(shortest), seconds(longest))
        } else {
            format!("of {} s at most", seconds(longest))
        };
        return refuse(format!("{what} {range}; {name} runs {secs:.1} s"));
    }
    Ok(())
}

/// "2" or "2.5".
fn seconds(secs: f64) -> String {
    if secs.fract() == 0.0 {
        format!("{secs:.0}")
    } else {
        format!("{secs:.1}")
    }
}

fn refuse<T>(message: String) -> CloudResult<T> {
    Err(CloudError::BadPrompt {
        message,
        suggestion: None,
    })
}
