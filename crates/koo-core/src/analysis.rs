//! Results of local analysis (PRD M3). All times are *source* times; map to
//! the timeline with [`crate::model::Clip::to_timeline`].

use crate::ids::SpeakerId;
use crate::model::Fingerprint;
use crate::time::{Time, TimeRange};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Word {
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
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Loudness {
    pub integrated_lufs: f32,
    pub true_peak_db: f32,
    pub range_lu: f32,
}
