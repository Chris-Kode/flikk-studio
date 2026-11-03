//! Transitions as timeline objects (what is drawn where, which clip edge
//! stores it) and where FX library drops land.

use flikk_core::command::Edge;
use flikk_core::fx::{self, PlacedTransition, TransitionAlign};
use flikk_core::model::{Clip, Track, TrackKind};
use flikk_core::{ClipId, Time, TimeRange};

/// Default length of a dropped transition.
pub const TRANSITION_SECS: f64 = 1.0;
/// Default length of a dropped generator.
pub const GENERATOR_SECS: f64 = 5.0;

/// Where a transition sits relative to its clips.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Place {
    /// Between two adjacent clips.
    Cut(TransitionAlign),
    /// At a clip start with nothing before it: inside the clip.
    Head,
    /// At a clip end with nothing after it: inside the clip.
    Tail,
}

/// A transition as drawn on a track.
#[derive(Clone, Debug)]
pub struct Placed {
    /// The clip edge that stores it (`transition_in` = start).
    pub owner: (ClipId, Edge),
    /// The cut, or the clip edge.
    pub at: Time,
    pub range: TimeRange,
    pub place: Place,
    /// Longest duration the clips around it allow.
    pub max: Time,
}

impl Placed {
    /// The range it would cover with duration `d`, as
    /// [`flikk_core::fx::transitions`] places it.
    pub fn range_with(&self, d: Time) -> TimeRange {
        let (at, d) = (self.at, d.max(Time::ZERO));
        match self.place {
            Place::Cut(TransitionAlign::Center) => {
                let half = Time(d.0 / 2);
                TimeRange::new(at - half, at - half + d)
            }
            Place::Cut(TransitionAlign::StartOnCut) | Place::Head => TimeRange::new(at, at + d),
            Place::Cut(TransitionAlign::EndOnCut) | Place::Tail => TimeRange::new(at - d, at),
        }
    }

    /// Duration that puts an edge of the block at `t`.
    pub fn duration_at(&self, t: Time) -> Time {
        let reach = Time((t - self.at).0.abs());
        match self.place {
            Place::Cut(TransitionAlign::Center) => Time(reach.0 * 2),
            _ => reach,
        }
    }
}

/// Every transition on `track`, with its owner.
pub fn placed(track: &Track) -> Vec<Placed> {
    fx::transitions(track)
        .iter()
        .filter_map(|p| {
            let owner = owner(p)?;
            let (place, max) = match (p.from, p.to) {
                (Some(a), Some(b)) => {
                    let align = p.transition.align;
                    let max = match align {
                        TransitionAlign::Center => Time(a.duration.min(b.duration).0 * 2),
                        TransitionAlign::StartOnCut => b.duration,
                        TransitionAlign::EndOnCut => a.duration,
                    };
                    (Place::Cut(align), max)
                }
                (None, Some(b)) => (Place::Head, b.duration),
                (Some(a), None) => (Place::Tail, a.duration),
                (None, None) => return None,
            };
            Some(Placed {
                owner,
                at: p.at,
                range: p.range,
                place,
                max,
            })
        })
        .collect()
}

fn owner(p: &PlacedTransition) -> Option<(ClipId, Edge)> {
    if let Some(to) = p.to
        && to
            .transition_in
            .as_ref()
            .is_some_and(|t| std::ptr::eq(t, p.transition))
    {
        return Some((to.id, Edge::Start));
    }
    p.from.map(|from| (from.id, Edge::End))
}

/// Tracks whose clips take transitions and generators.
pub fn takes_pictures(kind: TrackKind) -> bool {
    matches!(
        kind,
        TrackKind::Video | TrackKind::Overlay | TrackKind::Title
    )
}

/// Where a transition dropped at `t` goes: the nearest clip edge within
/// `threshold`, the clip and edge to store it on, and the range it would
/// cover with the default length.
pub struct EdgeTarget {
    pub owner: (ClipId, Edge),
    pub duration: Time,
    pub preview: TimeRange,
}

pub fn edge_target(track: &Track, t: Time, threshold: Time) -> Option<EdgeTarget> {
    if !takes_pictures(track.kind) {
        return None;
    }
    let clips: Vec<&Clip> = track.clips.iter().filter(|c| c.enabled).collect();
    let (i, edge, at) = clips
        .iter()
        .enumerate()
        .flat_map(|(i, c)| [(i, Edge::Start, c.start), (i, Edge::End, c.end())])
        .filter(|(_, _, at)| (at.0 - t.0).abs() <= threshold.0)
        .min_by_key(|(_, _, at)| (at.0 - t.0).abs())?;
    let want = Time::from_secs_f64(TRANSITION_SECS);
    let clip = clips[i];
    // A cut between adjacent clips: stored on the incoming clip.
    let cut = match edge {
        Edge::Start => i
            .checked_sub(1)
            .map(|p| clips[p])
            .filter(|p| p.end() == clip.start)
            .map(|p| (p, clip)),
        Edge::End => clips
            .get(i + 1)
            .copied()
            .filter(|n| n.start == clip.end())
            .map(|n| (clip, n)),
    };
    Some(match cut {
        Some((a, b)) => {
            let d = want.min(a.duration).min(b.duration);
            let half = Time(d.0 / 2);
            EdgeTarget {
                owner: (b.id, Edge::Start),
                duration: d,
                preview: TimeRange::new(at - half, at - half + d),
            }
        }
        None => {
            let d = want.min(clip.duration);
            EdgeTarget {
                owner: (clip.id, edge),
                duration: d,
                preview: match edge {
                    Edge::Start => TimeRange::new(at, at + d),
                    Edge::End => TimeRange::new(at - d, at),
                },
            }
        }
    })
}

/// The free span a generator dropped at `at` would fill: up to `want`
/// long, stopping at the next clip. `None` when `at` is on a clip.
pub fn free_span(track: &Track, at: Time, want: Time) -> Option<TimeRange> {
    if track.clips.iter().any(|c| c.start <= at && at < c.end()) {
        return None;
    }
    let end = track
        .clips
        .iter()
        .map(|c| c.start)
        .filter(|s| *s > at)
        .min()
        .map_or(at + want, |next| next.min(at + want));
    let range = TimeRange::new(at, end);
    (!range.is_empty()).then_some(range)
}
