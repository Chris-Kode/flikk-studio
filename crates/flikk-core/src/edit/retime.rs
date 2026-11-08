//! Clip speed and speed ramps.

use super::bounds::min_len;
use super::diff::diff_track;
use super::ops;
use super::slice::{replace_shifting, stretch_keyframes, with_duration};
use super::target::{clip_index, editable_clip};
use crate::command::Plan;
use crate::error::{CoreError, Result};
use crate::ids::*;
use crate::model::*;
use crate::speed::{MAX_SPEED, MIN_SPEED, SpeedKey};
use crate::time::Time;

/// Whether a clip or ramp key may play at `speed`: finite and within
/// [`MIN_SPEED`]..=[`MAX_SPEED`].
pub(super) fn valid_speed(speed: f64) -> bool {
    speed.is_finite() && (MIN_SPEED..=MAX_SPEED).contains(&speed)
}

/// The error for a speed [`valid_speed`] refuses.
pub(super) fn speed_error() -> CoreError {
    CoreError::Invalid(format!("speed must be between {MIN_SPEED} and {MAX_SPEED}"))
}

/// Retime a media clip: the source it plays stays the same, so the clip
/// lasts `duration × old / new`. Keyframes stretch with it.
pub(super) fn set_speed(project: &Project, id: ClipId, speed: f64, ripple: bool) -> Result<Plan> {
    let tl = &project.timeline;
    let (track, clip) = editable_clip(tl, id)?;
    if !valid_speed(speed) {
        return Err(speed_error());
    }
    if clip.media().is_none() {
        return Err(CoreError::Invalid("only media clips change speed".into()));
    }
    let ratio = clip.speed / speed;
    let mut duration = clip.duration.scale(ratio).max(min_len(tl));
    let ix = clip_index(track, id);
    if !ripple && let Some(next) = track.clips.get(ix + 1) {
        duration = duration.min(next.start - clip.start);
    }
    let mut c = clip.clone();
    c.speed = speed;
    c.duration = duration;
    // A ramp keeps its shape at the new pace.
    for k in &mut c.retime.keys {
        k.at = k.at.scale(ratio);
        k.speed = (k.speed * speed / clip.speed).clamp(MIN_SPEED, MAX_SPEED);
    }
    if c.is_ramped() {
        c.settle_ramp();
    }
    stretch_keyframes(&mut c, ratio);
    c.audio.fade_in = c.audio.fade_in.min(duration);
    c.audio.fade_out = c.audio.fade_out.min(duration);
    let shift = if ripple {
        duration - clip.duration
    } else {
        Time::ZERO
    };
    Ok(ops(diff_track(
        track,
        replace_shifting(track, ix, c, shift),
    )))
}

/// Give a media clip a speed ramp (see
/// [`Command::SetSpeedRamp`](crate::command::Command::SetSpeedRamp)). The
/// source it plays stays; keyframes stretch with its length.
pub(super) fn set_speed_ramp(
    project: &Project,
    id: ClipId,
    points: &[[f64; 2]],
    keep_length: bool,
    ripple: bool,
) -> Result<Plan> {
    let tl = &project.timeline;
    let (track, clip) = editable_clip(tl, id)?;
    if clip.media().is_none() {
        return Err(CoreError::Invalid("only media clips change speed".into()));
    }
    let mut points = crate::speed::clean_points(points).map_err(CoreError::Invalid)?;
    let source = clip.source_length().as_secs_f64();
    let duration = clip.duration.as_secs_f64().max(1e-9);
    let flat = points.windows(2).all(|w| (w[0][1] - w[1][1]).abs() < 1e-9);
    let mut c = clip.clone();
    let length = if points.len() < 2 || flat {
        // One speed again: the one given, else what the clip averages.
        c.retime.keys.clear();
        c.speed = match points.first() {
            Some(p) if !keep_length => p[1],
            _ => clip.speed,
        };
        Time::from_secs_f64(source / c.speed)
    } else {
        let mut m = crate::speed::shape_mean(&points);
        if keep_length {
            let scale = (source / duration) / m;
            for p in &mut points {
                p[1] = (p[1] * scale).clamp(MIN_SPEED, MAX_SPEED);
            }
            m = crate::speed::shape_mean(&points);
        }
        let length = Time::from_secs_f64(source / m.max(MIN_SPEED));
        c.retime.keys = points
            .iter()
            .map(|[u, s]| SpeedKey {
                at: length.scale(*u),
                speed: *s,
            })
            .collect();
        length
    };
    let length = length.max(min_len(tl));
    let ratio = length.as_secs_f64() / duration;
    stretch_keyframes(&mut c, ratio);
    let ix = clip_index(track, id);
    let mut end = length;
    if !ripple && let Some(next) = track.clips.get(ix + 1) {
        end = end.min(next.start - clip.start);
    }
    c.duration = length;
    let mut c = if end < length {
        with_duration(&c, end)
    } else {
        c
    };
    c.settle_ramp();
    c.audio.fade_in = c.audio.fade_in.min(c.duration);
    c.audio.fade_out = c.audio.fade_out.min(c.duration);
    let shift = if ripple {
        c.duration - clip.duration
    } else {
        Time::ZERO
    };
    Ok(ops(diff_track(
        track,
        replace_shifting(track, ix, c, shift),
    )))
}
