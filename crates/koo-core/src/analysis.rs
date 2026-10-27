//! Results of local analysis (PRD M3). All times are *source* times; map to
//! the timeline with [`crate::model::Clip::to_timeline`].

use crate::ids::SpeakerId;
use crate::model::Fingerprint;
use crate::time::{FLICKS_PER_SECOND, Time, TimeRange};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Bumped when analysis learns something older results lack; those are
/// upgraded the next time the item is analyzed.
pub const ANALYSIS_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Word {
    /// Where the word sounds: Whisper's estimate, moved onto the audio
    /// (from version 1) so that its edges sit at the pauses around it.
    pub range: TimeRange,
    pub text: String,
    /// 0..1 from the recognizer; the caption pass flags low values (PRD recipes).
    pub confidence: f32,
    pub speaker: Option<SpeakerId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Speaker {
    pub id: SpeakerId,
    /// "Speaker 1" until confirmed by a source fact.
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    pub fingerprint: Fingerprint,
    /// Position in `MediaInfo::audio` (ffmpeg `0:a:N`), not the container index.
    pub audio_stream: u32,
    pub language: String,
    pub words: Vec<Word>,
    pub speakers: Vec<Speaker>,
}

impl Transcript {
    pub fn words_in(&self, range: TimeRange) -> impl Iterator<Item = &Word> {
        self.words.iter().filter(move |w| w.range.overlaps(&range))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Analysis {
    pub transcript: Option<Transcript>,
    /// Speech regions from voice activity detection.
    pub speech: Vec<TimeRange>,
    pub shot_changes: Vec<Time>,
    /// Loudness of the analyzed audio stream, for placing cuts in pauses.
    #[serde(default)]
    pub levels: Option<Levels>,
    /// The [`ANALYSIS_VERSION`] that made it; 0 before versions existed.
    #[serde(default)]
    pub version: u32,
}

/// Audio level in 10 ms frames from the start of the source, in whole dB
/// below full scale. Stored as hex, one byte per frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Levels {
    /// −dBFS per frame, 0..=100.
    below_full_scale: Vec<u8>,
}

impl Levels {
    pub const FRAME: Time = Time(FLICKS_PER_SECOND / 100);

    pub fn from_db(db: impl IntoIterator<Item = f32>) -> Self {
        Self {
            below_full_scale: db
                .into_iter()
                .map(|d| (-d).round().clamp(0.0, 100.0) as u8)
                .collect(),
        }
    }

    pub fn len(&self) -> usize {
        self.below_full_scale.len()
    }

    pub fn is_empty(&self) -> bool {
        self.below_full_scale.is_empty()
    }

    /// dBFS of frame `i`.
    pub fn db(&self, i: usize) -> f32 {
        -(self.below_full_scale[i] as f32)
    }

    /// dBFS of the frame playing at source time `t`, if the audio reaches it.
    pub fn at(&self, t: Time) -> Option<f32> {
        let i = usize::try_from(t.0.div_euclid(Self::FRAME.0)).ok()?;
        (i < self.len()).then(|| self.db(i))
    }
}

impl Serialize for Levels {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use std::fmt::Write;
        let mut hex = String::with_capacity(self.len() * 2);
        for b in &self.below_full_scale {
            let _ = write!(hex, "{b:02x}");
        }
        s.serialize_str(&hex)
    }
}

impl<'de> Deserialize<'de> for Levels {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let hex = String::deserialize(d)?;
        let bytes = hex
            .as_bytes()
            .chunks(2)
            .map(|pair| {
                std::str::from_utf8(pair)
                    .ok()
                    .and_then(|p| u8::from_str_radix(p, 16).ok())
                    .ok_or_else(|| serde::de::Error::custom("levels are not hex"))
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            below_full_scale: bytes,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Loudness {
    pub integrated_lufs: f32,
    pub true_peak_db: f32,
    pub range_lu: f32,
}
