//! Speaker attribution (PRD P1). A speakers model from `koo-models` tells
//! voices apart when the `diarizer` feature is on; [`SingleSpeaker`]
//! labels every word "Speaker 1" otherwise, and when the model fails.
//! Another diarizer implements [`Diarizer`] and slots into
//! [`crate::LocalAnalysis::with_diarizer`].

use anyhow::Result;
use koo_core::analysis::{Speaker, Word};
use koo_core::ids::SpeakerId;
pub use koo_models::speakers::{Diarizer, Labels};

/// Words below this attribution confidence keep `speaker: None` (PRD F3).
pub const MIN_CONFIDENCE: f32 = 0.6;

/// One voice for the whole item.
pub struct SingleSpeaker;

impl Diarizer for SingleSpeaker {
    fn diarize(&self, _audio: &[f32], words: &[Word], _progress: &dyn Fn(f32)) -> Result<Labels> {
        Ok((1, words.iter().map(|_| Some((0, 1.0))).collect()))
    }
}

/// Write `labels` into `words` as speakers numbered in order of their
/// first word. A voice that takes over most of the words of a speaker the
/// words had before keeps that speaker's id, so what refers to it still
/// does.
pub fn apply(labels: Labels, words: &mut [Word]) -> Vec<Speaker> {
    let (count, labels) = labels;
    let labels: Vec<Option<usize>> = labels
        .into_iter()
        .map(|l| l.filter(|&(i, confidence)| confidence >= MIN_CONFIDENCE && i < count))
        .map(|l| l.map(|(i, _)| i))
        .collect();
    // Voices in order of appearance, unheard ones dropped.
    let mut order: Vec<usize> = Vec::new();
    for &i in labels.iter().flatten() {
        if !order.contains(&i) {
            order.push(i);
        }
    }
    let mut shared: Vec<(usize, SpeakerId, usize)> = Vec::new();
    for (word, label) in words.iter().zip(&labels) {
        let (Some(old), Some(new)) = (word.speaker, *label) else {
            continue;
        };
        match shared.iter_mut().find(|(n, o, _)| *n == new && *o == old) {
            Some(pair) => pair.2 += 1,
            None => shared.push((new, old, 1)),
        }
    }
    shared.sort_by_key(|pair| std::cmp::Reverse(pair.2));
    let mut ids: Vec<Option<SpeakerId>> = vec![None; count];
    for (new, old, _) in shared {
        if ids[new].is_none() && !ids.contains(&Some(old)) {
            ids[new] = Some(old);
        }
    }
    let speakers: Vec<Speaker> = order
        .iter()
        .enumerate()
        .map(|(n, &i)| Speaker {
            id: ids[i].unwrap_or_default(),
            label: format!("Speaker {}", n + 1),
        })
        .collect();
    for (word, label) in words.iter_mut().zip(labels) {
        word.speaker =
            label.map(|i| speakers[order.iter().position(|&o| o == i).expect("heard")].id);
    }
    speakers
}
