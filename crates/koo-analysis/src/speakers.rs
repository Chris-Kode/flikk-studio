//! Speaker attribution (PRD P1). The MVP labels every word "Speaker 1";
//! a real diarizer implements [`Diarizer`] and slots into
//! [`crate::LocalAnalysis::with_diarizer`].

use koo_core::analysis::{Speaker, Word};
use koo_core::ids::SpeakerId;

/// Words below this attribution confidence keep `speaker: None` (PRD F3).
pub const MIN_CONFIDENCE: f32 = 0.6;

/// Groups words by voice.
pub trait Diarizer: Send + Sync {
    /// Number of distinct voices and, for each word, the index of its voice
    /// with a 0..1 confidence (`None` when it cannot tell). `audio` is 16 kHz
    /// mono for the same source times as the words.
    fn diarize(&self, audio: &[f32], words: &[Word]) -> (usize, Vec<Option<(usize, f32)>>);
}

/// One voice for the whole item.
pub struct SingleSpeaker;

impl Diarizer for SingleSpeaker {
    fn diarize(&self, _audio: &[f32], words: &[Word]) -> (usize, Vec<Option<(usize, f32)>>) {
        (1, words.iter().map(|_| Some((0, 1.0))).collect())
    }
}

/// Run `diarizer` and write the speakers into `words`.
pub fn attribute(diarizer: &dyn Diarizer, audio: &[f32], words: &mut [Word]) -> Vec<Speaker> {
    if words.is_empty() {
        return Vec::new();
    }
    let (count, labels) = diarizer.diarize(audio, words);
    let speakers: Vec<Speaker> = (0..count)
        .map(|i| Speaker {
            id: SpeakerId::new(),
            label: format!("Speaker {}", i + 1),
        })
        .collect();
    for (word, label) in words.iter_mut().zip(labels) {
        word.speaker = label
            .filter(|&(i, confidence)| confidence >= MIN_CONFIDENCE && i < count)
            .map(|(i, _)| speakers[i].id);
    }
    // Only list voices that ended up attributed to something.
    speakers
        .into_iter()
        .filter(|s| words.iter().any(|w| w.speaker == Some(s.id)))
        .collect()
}
