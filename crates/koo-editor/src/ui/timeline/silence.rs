//! Silence removal (PRD E4): gaps from `AnalysisService::silences`, mapped
//! from source to timeline time through the clips, kept only where every
//! audible clip is silent, then removed as one undo step.

use koo_core::command::Command;
use koo_core::model::{ClipContent, Project, TrackKind};
use koo_core::services::AnalysisService;
use koo_core::{MediaId, Time, TimeRange};
use std::collections::HashMap;

/// Source silences per media item, for one minimum length.
#[derive(Default)]
pub struct SilenceCache {
    min: Time,
    /// `None` while in flight or after a failure.
    by_media: HashMap<MediaId, Option<Vec<TimeRange>>>,
}

impl SilenceCache {
    /// Media whose silences for `min` are not known or requested yet. Marks
    /// them as requested.
    pub fn missing(&mut self, project: &Project, min: Time) -> Vec<koo_core::model::MediaItem> {
        if self.min != min {
            self.min = min;
            self.by_media.clear();
        }
        let mut out = Vec::new();
        for clip in sound_clips(project) {
            let Some(id) = clip.1 else { continue };
            if self.by_media.contains_key(&id) {
                continue;
            }
            self.by_media.insert(id, None);
            if let Some(item) = project.media(id) {
                out.push(item.clone());
            }
        }
        out
    }

    pub fn insert(&mut self, min: Time, media: MediaId, ranges: Option<Vec<TimeRange>>) {
        if min == self.min {
            self.by_media.insert(media, ranges);
        }
    }

    pub fn is_loading(&self) -> bool {
        self.by_media.values().any(Option::is_none)
    }

    /// Timeline ranges to remove: inside clip coverage, silent on every
    /// audible clip, at least `min` long after shrinking by `padding` per side.
    pub fn timeline_silences(&self, project: &Project, min: Time, padding: Time) -> Vec<TimeRange> {
        let mut covered = Vec::new();
        let mut sound = Vec::new();
        for (clip, media) in sound_clips(project) {
            covered.push(clip.range());
            let silences = media
                .and_then(|m| self.by_media.get(&m))
                .and_then(|s| s.as_ref());
            let Some(silences) = silences else {
                // Unknown: treat the whole clip as sound.
                sound.push(clip.range());
                continue;
            };
            let ClipContent::Media { source_in, .. } = &clip.content else {
                continue;
            };
            let mut quiet: Vec<TimeRange> = silences
                .iter()
                .filter_map(|s| {
                    let start = clip.start + (s.start - *source_in);
                    let end = clip.start + (s.end - *source_in);
                    TimeRange::new(start, end).intersect(&clip.range())
                })
                .collect();
            quiet.sort_by_key(|r| r.start);
            sound.extend(subtract(clip.range(), &quiet));
        }
        let covered = union(covered);
        let sound = union(sound);
        let mut out = Vec::new();
        for c in covered {
            for gap in subtract(c, &sound) {
                let r = TimeRange::new(gap.start + padding, gap.end - padding);
                if gap.duration() >= min && !r.is_empty() {
                    out.push(r);
                }
            }
        }
        out
    }
}

/// `RippleDelete`s for `ranges`, latest first so earlier ranges stay valid.
pub fn removal_commands(ranges: &[TimeRange]) -> Vec<Command> {
    let mut ranges = ranges.to_vec();
    ranges.sort_by_key(|r| std::cmp::Reverse(r.start));
    ranges
        .into_iter()
        .map(|range| Command::RippleDelete {
            range,
            tracks: None,
        })
        .collect()
}

/// Run on a background thread.
pub fn detect(
    analysis: &dyn AnalysisService,
    item: &koo_core::model::MediaItem,
    min: Time,
) -> Option<Vec<TimeRange>> {
    analysis.silences(item, min).ok()
}

/// Media clips whose sound decides silence: audio tracks, or video tracks
/// when the timeline has no audio clips.
fn sound_clips(project: &Project) -> Vec<(&koo_core::model::Clip, Option<MediaId>)> {
    let pick = |kind: TrackKind| -> Vec<_> {
        project
            .timeline
            .tracks
            .iter()
            .filter(|t| t.kind == kind && !t.mix.mute)
            .flat_map(|t| t.clips.iter())
            .filter(|c| c.media().is_some())
            .map(|c| (c, c.media()))
            .collect()
    };
    let audio = pick(TrackKind::Audio);
    if audio.is_empty() {
        pick(TrackKind::Video)
    } else {
        audio
    }
}

fn union(mut ranges: Vec<TimeRange>) -> Vec<TimeRange> {
    ranges.sort_by_key(|r| r.start);
    let mut out: Vec<TimeRange> = Vec::new();
    for r in ranges {
        match out.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}

/// `range` minus sorted, non-overlapping `holes`.
fn subtract(range: TimeRange, holes: &[TimeRange]) -> Vec<TimeRange> {
    let mut out = Vec::new();
    let mut cursor = range.start;
    for h in holes {
        if h.end <= cursor || h.start >= range.end {
            continue;
        }
        if h.start > cursor {
            out.push(TimeRange::new(cursor, h.start));
        }
        cursor = cursor.max(h.end);
    }
    if cursor < range.end {
        out.push(TimeRange::new(cursor, range.end));
    }
    out
}
