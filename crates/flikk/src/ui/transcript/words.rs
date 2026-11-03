//! Transcript words placed on the timeline, through the shared core mapping.

use flikk_core::analysis::Word;
use flikk_core::cuts::Speech;
use flikk_core::model::Project;
use flikk_core::query::{self, TimelineWord};
use flikk_core::services::AnalysisService;
use flikk_core::{SpeakerId, Time, TimeRange};
use std::collections::HashMap;

/// Every transcribed word on the timeline, in timeline order, with speaker
/// labels. A word cut by a clip edge belongs to the clip holding most of it.
pub fn timeline_words(
    project: &Project,
    analysis: &dyn AnalysisService,
) -> (Vec<TimelineWord>, HashMap<SpeakerId, String>) {
    let all = TimeRange::new(Time::ZERO, project.timeline.duration());
    let mut words = query::words_in_range(project, all, |fp| analysis.analysis(fp));
    words.retain(|w| {
        !w.word.text.trim().is_empty() && w.timeline.duration().0 * 2 >= w.source.duration().0
    });
    for w in &mut words {
        w.word.text = w.word.text.trim().to_owned();
    }
    let speakers = project
        .media
        .iter()
        .filter_map(|m| m.fingerprint.as_ref())
        .filter_map(|fp| analysis.analysis(fp))
        .filter_map(|a| a.transcript.as_ref().map(|t| t.speakers.clone()))
        .flatten()
        .map(|s| (s.id, s.label))
        .collect();
    (words, speakers)
}

/// As a recognizer word in timeline time, for caption grouping.
pub fn to_word(w: &TimelineWord) -> Word {
    Word {
        range: w.timeline,
        ..w.word.clone()
    }
}

/// Timeline ranges to ripple-delete for the words `first..=last`: cut in
/// the pauses around them, so the pause after them goes too, and returned
/// last first so earlier deletes don't shift later ones.
pub fn delete_ranges(
    project: &Project,
    analysis: &dyn AnalysisService,
    words: &[TimelineWord],
    first: usize,
    last: usize,
) -> Vec<TimeRange> {
    Speech::new(project, |fp| analysis.analysis(fp))
        .removals(words, first, last)
        .into_iter()
        .map(|r| r.range)
        .collect()
}
