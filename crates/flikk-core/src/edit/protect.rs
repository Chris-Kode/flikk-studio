//! Warnings for plans that change protected ranges (PRD F4).

use crate::command::{Plan, Warning};
use crate::ids::*;
use crate::model::*;
use crate::ops::apply_all;
use crate::time::{Time, TimeRange};

/// Add [`Warning::TouchesProtected`] for every protected marker whose content
/// the plan removes, shifts or replaces (PRD F4). A marker is untouched when,
/// after the plan, it still exists with the same length and every piece of
/// media it covered still plays at the same offset inside it. Adding content
/// over a protected range, or moving the marker together with its content,
/// does not warn.
pub(super) fn warn_protected(project: &Project, plan: &mut Plan) {
    let protected: Vec<&Marker> = project
        .timeline
        .markers
        .iter()
        .filter(|m| m.kind == MarkerKind::Protected && !m.range.is_empty())
        .collect();
    if protected.is_empty() || plan.ops.is_empty() {
        return;
    }
    let mut after = project.clone();
    if apply_all(&mut after, &plan.ops).is_err() {
        return;
    }
    for m in protected {
        let touched = match after.timeline.marker(m.id) {
            Some(n) if n.range.duration() == m.range.duration() => {
                project.timeline.tracks.iter().any(|t| {
                    let now = after.timeline.track(t.id).map_or(&[][..], |t| &t.clips);
                    let kept = segments(now, n.range);
                    segments(&t.clips, m.range).iter().any(|old| {
                        !kept.iter().any(|new| {
                            new.key == old.key && new.start <= old.start && old.end <= new.end
                        })
                    })
                })
            }
            _ => true,
        };
        let known = plan
            .warnings
            .iter()
            .any(|w| matches!(w, Warning::TouchesProtected { marker, .. } if *marker == m.id));
        if touched && !known {
            plan.warnings.push(Warning::TouchesProtected {
                marker: m.id,
                note: m.note.clone(),
            });
        }
    }
}

#[derive(PartialEq)]
enum SegmentKey<'a> {
    /// Source time minus offset into the range: equal for continuous media.
    Media {
        media: MediaId,
        stream: Option<u32>,
        offset: Time,
        speed: u64,
    },
    Other(&'a ClipContent),
}

struct Segment<'a> {
    /// Relative to the range start.
    start: Time,
    end: Time,
    key: SegmentKey<'a>,
}

/// What plays inside `range`, relative to its start, with continuous pieces
/// (e.g. the two halves of a blade) merged.
fn segments(clips: &[Clip], range: TimeRange) -> Vec<Segment<'_>> {
    let mut out: Vec<Segment> = Vec::new();
    for c in clips {
        let Some(r) = c.range().intersect(&range) else {
            continue;
        };
        let key = match &c.content {
            ClipContent::Media {
                media,
                source_in,
                audio_stream,
            } => SegmentKey::Media {
                media: *media,
                stream: *audio_stream,
                offset: *source_in - c.source_offset(c.start - range.start),
                speed: c.speed.to_bits(),
            },
            other => SegmentKey::Other(other),
        };
        let (start, end) = (r.start - range.start, r.end - range.start);
        match out.last_mut() {
            Some(last) if last.end == start && last.key == key => last.end = end,
            _ => out.push(Segment { start, end, key }),
        }
    }
    out
}
