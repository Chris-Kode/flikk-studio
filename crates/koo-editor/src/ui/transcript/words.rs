//! Transcript words placed on the timeline: clip media → fingerprint →
//! analysis → words, mapped from source to timeline time through each clip.
//!
//! A local stand-in for the shared mapping helper the core stream is adding.

use koo_core::analysis::Word;
use koo_core::model::{Project, TrackKind};
use koo_core::services::AnalysisService;
use koo_core::{ClipId, MediaId, SpeakerId, Time, TimeRange};
use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq)]
pub struct TimelineWord {
    /// Timeline time, clipped to the clip.
    pub range: TimeRange,
    /// Source time of the whole word.
    pub source: TimeRange,
    pub media: MediaId,
    pub clip: ClipId,
    pub text: String,
    pub confidence: f32,
    pub speaker: Option<SpeakerId>,
    pub speaker_label: Option<String>,
}

impl TimelineWord {
    /// As a recognizer word in timeline time, for caption grouping.
    pub fn to_word(&self) -> Word {
        Word {
            range: self.range,
            text: self.text.clone(),
            confidence: self.confidence,
            speaker: self.speaker,
        }
    }
}

/// Every transcribed word under a media clip, in timeline order. A clip whose
/// media is already placed at the same offset on another track (linked video
/// and audio) is read once.
pub fn timeline_words(project: &Project, analysis: &dyn AnalysisService) -> Vec<TimelineWord> {
    let mut out = Vec::new();
    let mut seen: HashSet<(MediaId, Time, Time)> = HashSet::new();
    let tracks = project
        .timeline
        .tracks
        .iter()
        .filter(|t| matches!(t.kind, TrackKind::Audio | TrackKind::Video));
    for track in tracks {
        for clip in &track.clips {
            let Some(media_id) = clip.media() else {
                continue;
            };
            let Some(source_in) = clip.to_source(clip.start) else {
                continue;
            };
            if !seen.insert((media_id, clip.start - source_in, clip.start)) {
                continue;
            }
            let Some(fp) = project.media(media_id).and_then(|m| m.fingerprint.as_ref()) else {
                continue;
            };
            let Some(result) = analysis.analysis(fp) else {
                continue;
            };
            let Some(transcript) = &result.transcript else {
                continue;
            };
            let source = TimeRange::new(source_in, source_in + clip.duration);
            for w in transcript.words_in(source) {
                // A word belongs to the clip that holds its midpoint.
                let mid = Time(w.range.start.0 + w.range.duration().0 / 2);
                if !source.contains(mid) {
                    continue;
                }
                let shift = clip.start - source_in;
                let Some(range) = w.range.shift(shift).intersect(&clip.range()) else {
                    continue;
                };
                out.push(TimelineWord {
                    range,
                    source: w.range,
                    media: media_id,
                    clip: clip.id,
                    text: w.text.trim().to_owned(),
                    confidence: w.confidence,
                    speaker: w.speaker,
                    speaker_label: w.speaker.and_then(|s| {
                        transcript
                            .speakers
                            .iter()
                            .find(|sp| sp.id == s)
                            .map(|sp| sp.label.clone())
                    }),
                });
            }
        }
    }
    out.retain(|w| !w.text.is_empty());
    out.sort_by_key(|w| (w.range.start, w.range.end));
    out
}

/// Timeline ranges to ripple-delete for the words `first..=last`, one per
/// clip, each running to the next word's onset so the pause goes too.
/// Returned in descending order, so earlier deletes don't shift later ones.
pub fn delete_ranges(words: &[TimelineWord], first: usize, last: usize) -> Vec<TimeRange> {
    let mut ranges: Vec<TimeRange> = Vec::new();
    let mut i = first;
    while i <= last && i < words.len() {
        let clip = words[i].clip;
        let start = words[i].range.start;
        let mut j = i;
        while j < last && words[j + 1].clip == clip {
            j += 1;
        }
        let end = match words.get(j + 1) {
            Some(next) if next.clip == clip && next.range.start >= words[j].range.end => {
                next.range.start
            }
            _ => words[j].range.end,
        };
        ranges.push(TimeRange::new(start, end));
        i = j + 1;
    }
    ranges.retain(|r| !r.is_empty());
    ranges.sort_by_key(|r| std::cmp::Reverse(r.start));
    ranges
}
