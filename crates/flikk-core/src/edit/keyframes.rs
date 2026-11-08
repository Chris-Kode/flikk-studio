//! Clip property keyframes: zooms, set and remove.

use super::bounds::min_len;
use super::target::find_clip;
use super::update::update_clip;
use crate::command::Plan;
use crate::error::{CoreError, Result};
use crate::ids::*;
use crate::model::*;
use crate::time::{FLICKS_PER_SECOND, Time, TimeRange};

/// Longest ease in and out of a zoom; shorter ranges use 20% of their length.
const ZOOM_RAMP: Time = Time(FLICKS_PER_SECOND * 2 / 5);

/// Keyframes `Scale`, and `PositionX/Y` unless `target` is the centre, so the
/// clip eases from its current value to `scale` over the ramp, holds, then
/// eases back by `range.end`. `target` is a point in the normalized space of
/// [`Transform::position`] ((0, 0) = centre); position is keyframed to
/// `-target * (scale - 1)` so that point stays in place while the frame grows
/// around it. Existing keyframes of those properties inside the range go.
pub(super) fn add_zoom(
    tl: &Timeline,
    id: ClipId,
    range: TimeRange,
    target: [f32; 2],
    scale: f32,
    ease: Ease,
) -> Result<Plan> {
    let (_, clip) = find_clip(tl, id)?;
    if !(scale.is_finite() && scale > 0.0) || !target.iter().all(|v| v.is_finite()) {
        return Err(CoreError::Invalid("invalid zoom scale or target".into()));
    }
    let r = range
        .intersect(&clip.range())
        .filter(|r| r.duration() >= min_len(tl))
        .ok_or_else(|| CoreError::Invalid("the zoom range is outside the clip".into()))?;
    let (a, b) = (r.start - clip.start, r.end - clip.start);
    let ramp = ZOOM_RAMP.min(Time(r.duration().0 / 5));
    let mut props = vec![Property::Scale];
    if target != [0.0, 0.0] {
        props.extend([Property::PositionX, Property::PositionY]);
    }
    let peak = |property: Property, from: f32| match property {
        Property::Scale => scale,
        Property::PositionX => from - target[0] * (scale - 1.0),
        _ => from - target[1] * (scale - 1.0),
    };
    let keys: Vec<Keyframe> = props
        .iter()
        .flat_map(|&property| {
            let (from, to) = (clip.value_at(property, a), clip.value_at(property, b));
            let peak = peak(property, from);
            [
                (a, from, ease),
                (a + ramp, peak, Ease::Linear),
                (b - ramp, peak, ease),
                (b, to, ease),
            ]
            .map(|(at, value, ease)| Keyframe {
                property,
                at,
                value,
                ease,
            })
        })
        .collect();
    update_clip(tl, id, |c| {
        c.keyframes
            .retain(|k| !(props.contains(&k.property) && a <= k.at && k.at <= b));
        c.keyframes.extend(keys);
        c.keyframes.sort_by_key(|k| k.at);
    })
}

/// `at` is timeline time, like every other command; it is stored relative to
/// the clip's start and must fall inside the clip.
pub(super) fn set_keyframe(
    tl: &Timeline,
    id: ClipId,
    property: Property,
    at: Time,
    value: f32,
    ease: Ease,
) -> Result<Plan> {
    let (_, clip) = find_clip(tl, id)?;
    let rel = at - clip.start;
    if rel < Time::ZERO || rel > clip.duration {
        return Err(CoreError::Invalid(
            "the keyframe is outside the clip".into(),
        ));
    }
    if !value.is_finite() {
        return Err(CoreError::Invalid("invalid keyframe value".into()));
    }
    update_clip(tl, id, |c| {
        c.keyframes
            .retain(|k| !(k.property == property && k.at == rel));
        c.keyframes.push(Keyframe {
            property,
            at: rel,
            value,
            ease,
        });
        c.keyframes.sort_by_key(|k| k.at);
    })
}

/// Removes the keyframe of `property` nearest to timeline time `at`, within
/// half a frame.
pub(super) fn remove_keyframe(
    tl: &Timeline,
    id: ClipId,
    property: Property,
    at: Time,
) -> Result<Plan> {
    let (_, clip) = find_clip(tl, id)?;
    let rel = at - clip.start;
    let ix = nearest_key(
        clip.keyframes
            .iter()
            .map(|k| (k.property == property).then_some(k.at)),
        rel,
        key_tolerance(tl),
    )
    .ok_or_else(|| CoreError::Invalid(format!("no {property:?} keyframe at {at}")))?;
    update_clip(tl, id, |c| {
        c.keyframes.remove(ix);
    })
}

/// How far from a requested time a keyframe may sit and still be the one
/// meant: half a sequence frame.
pub(super) fn key_tolerance(tl: &Timeline) -> Time {
    Time(tl.settings.rate.frame_duration().0 / 2)
}

/// Index of the keyframe time nearest to `rel` and at most `tolerance` from
/// it; the first of equally near ones wins. `times` holds one entry per
/// keyframe, `None` for those that don't count (another property).
pub(super) fn nearest_key(
    times: impl Iterator<Item = Option<Time>>,
    rel: Time,
    tolerance: Time,
) -> Option<usize> {
    times
        .enumerate()
        .filter_map(|(i, at)| Some((i, (at? - rel).0.abs())))
        .filter(|(_, d)| *d <= tolerance.0)
        .min_by_key(|(_, d)| *d)
        .map(|(i, _)| i)
}
