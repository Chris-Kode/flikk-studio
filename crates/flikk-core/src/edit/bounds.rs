//! How far an edit may move a clip edge: source media and frame limits.

use crate::ids::*;
use crate::model::*;
use crate::time::Time;

/// Shortest clip a trim, roll or slide leaves behind: one sequence frame.
pub(super) fn min_len(tl: &Timeline) -> Time {
    tl.settings.rate.frame_duration()
}

/// Known length of a pooled source; `None` for stills and unprobed media.
pub(crate) fn media_duration(project: &Project, id: MediaId) -> Option<Time> {
    project
        .media(id)
        .filter(|m| m.kind != MediaKind::Image)
        .and_then(|m| m.info.as_ref())
        .map(|i| i.duration)
        .filter(|d| *d > Time::ZERO)
}

/// Source media available before the clip's first frame, in timeline time
/// at the clip's speed; `None` = unlimited.
pub(super) fn room_before(clip: &Clip) -> Option<Time> {
    match &clip.content {
        ClipContent::Media { source_in, .. } => {
            Some(-clip.timeline_offset(-(*source_in).max(Time::ZERO)))
        }
        _ => None,
    }
}

/// Source media available after the clip's last frame, in timeline time at
/// the clip's speed; `None` = unlimited or unknown.
pub(super) fn room_after(project: &Project, clip: &Clip) -> Option<Time> {
    let ClipContent::Media {
        media, source_in, ..
    } = &clip.content
    else {
        return None;
    };
    media_duration(project, *media).map(|d| {
        let left = (d - *source_in - clip.source_length()).max(Time::ZERO);
        (clip.timeline_offset(clip.source_length() + left) - clip.duration).max(Time::ZERO)
    })
}

/// The tighter of two optional lower bounds.
pub(super) fn lower(a: Option<Time>, b: Option<Time>) -> Option<Time> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

/// The tighter of two optional upper bounds.
pub(super) fn upper(a: Option<Time>, b: Option<Time>) -> Option<Time> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// Clamp `delta` into `[lo, hi]`; bounds never exclude zero, so a clip that is
/// already past a limit simply cannot move further that way.
pub(super) fn clamp(delta: Time, lo: Option<Time>, hi: Option<Time>) -> Time {
    let mut d = delta;
    if let Some(hi) = hi {
        d = d.min(hi.max(Time::ZERO));
    }
    if let Some(lo) = lo {
        d = d.max(lo.min(Time::ZERO));
    }
    d
}
