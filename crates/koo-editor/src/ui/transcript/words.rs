//! Transcript words placed on the timeline, through the shared core mapping.

use koo_core::analysis::Word;
use koo_core::model::Project;
use koo_core::query::{self, TimelineWord};
use koo_core::services::AnalysisService;
use koo_core::{SpeakerId, Time, TimeRange};
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

/// Timeline ranges to ripple-delete for the words `first..=last`: each word
/// runs to the next word's onset in the same clip, so the pause goes too.
/// Merged, and returned last first so earlier deletes don't shift later ones.
pub fn delete_ranges(words: &[TimelineWord], first: usize, last: usize) -> Vec<TimeRange> {
    let ranges = (first..=last.min(words.len().saturating_sub(1)))
        .map(|i| {
            let w = &words[i];
            let end = match words.get(i + 1) {
                Some(next) if next.clip == w.clip && next.timeline.start >= w.timeline.end => {
                    next.timeline.start
                }
                _ => w.timeline.end,
            };
            TimeRange::new(w.timeline.start, end)
        })
        .collect();
    let mut merged = query::merge_ranges(ranges);
    merged.reverse();
    merged
}
