//! How a clip is cut: splits, trims, new lengths and stretches, with
//! their keyframes, ramps and fades.

use crate::ids::*;
use crate::model::*;
use crate::time::{Time, TimeRange};

/// Split at timeline time `at` (strictly inside the clip). The left half keeps
/// the id; the right half is new. Fades and keyframes go with their side,
/// and each half gets a keyframe at the cut so animated values don't jump.
pub(crate) fn split_clip(clip: &Clip, at: Time) -> (Clip, Clip) {
    let mut left = with_duration(clip, at - clip.start);
    left.audio.fade_out = Time::ZERO;
    left.transition_out = None;
    let mut right = trim_start(clip, at);
    right.id = ClipId::new();
    right.audio.fade_in = Time::ZERO;
    right.transition_in = None;
    // Each half owns its effect instances.
    for e in &mut right.effects {
        e.id = EffectId::new();
    }
    if let ClipContent::Generator { fx } = &mut right.content {
        fx.id = EffectId::new();
    }
    (left, right)
}

/// The same clip with its head trimmed so it starts at timeline time `at`.
pub(super) fn trim_start(clip: &Clip, at: Time) -> Clip {
    let offset = at - clip.start;
    let mut c = clip.clone();
    c.start = at;
    c.duration = clip.duration - offset;
    match &mut c.content {
        ClipContent::Media { source_in, .. } => {
            *source_in = *source_in + clip.source_offset(offset)
        }
        ClipContent::Generator { fx } => fx.shift_keyframes(-offset),
        ClipContent::Caption { words, .. } => {
            for w in words {
                *w = *w - offset;
            }
        }
        _ => {}
    }
    for e in &mut c.effects {
        e.shift_keyframes(-offset);
    }
    for m in &mut c.masks {
        if offset > Time::ZERO {
            m.keep_from(offset);
        } else {
            m.keyframes.iter_mut().for_each(|k| k.at = k.at - offset);
        }
    }
    c.keyframes = if offset > Time::ZERO {
        keyframes_from(clip, offset)
    } else {
        clip.keyframes
            .iter()
            .map(|k| Keyframe {
                at: k.at - offset,
                ..k.clone()
            })
            .collect()
    };
    if clip.is_ramped() {
        c.retime.keys = if offset > Time::ZERO {
            crate::speed::slice(&clip.retime.keys, offset, clip.duration)
        } else {
            let mut keys = clip.retime.keys.clone();
            keys.iter_mut().for_each(|k| k.at = k.at - offset);
            keys
        };
        c.settle_ramp();
    }
    c.audio.fade_in = c.audio.fade_in.min(c.duration);
    c.audio.fade_out = c.audio.fade_out.min(c.duration);
    c
}

/// The same clip with its tail set so it lasts `duration`. Keyframes past
/// the new end are replaced by one at the end holding the value there.
pub(super) fn with_duration(clip: &Clip, duration: Time) -> Clip {
    let mut c = clip.clone();
    c.duration = duration;
    if duration < clip.duration {
        c.keyframes = keyframes_until(clip, duration);
        for m in &mut c.masks {
            m.keep_until(duration);
        }
    }
    if c.is_ramped() {
        c.settle_ramp();
    }
    c.audio.fade_in = c.audio.fade_in.min(duration);
    c.audio.fade_out = c.audio.fade_out.min(duration);
    c
}

/// `clips` with everything inside `range` removed, without closing the gap.
/// A clip spanning the whole range is split; the right part gets a new id.
/// An edge that is cut away loses its transition.
pub(crate) fn cut_out(clips: &[Clip], range: TimeRange) -> Vec<Clip> {
    let mut next = Vec::with_capacity(clips.len() + 1);
    for c in clips {
        if c.end() <= range.start || c.start >= range.end {
            next.push(c.clone());
            continue;
        }
        if c.start < range.start {
            let mut left = with_duration(c, range.start - c.start);
            left.transition_out = None;
            next.push(left);
        }
        if c.end() > range.end {
            let mut right = trim_start(c, range.end);
            right.transition_in = None;
            if c.start < range.start {
                right.id = ClipId::new();
            }
            next.push(right);
        }
    }
    next
}

/// `clips` with a gap `len` long opened at timeline time `at`: clips from
/// `at` on move `len` later, and a clip across `at` is split there (see
/// [`split_clip`]) so its right half moves too.
pub(super) fn open_gap(clips: &[Clip], at: Time, len: Time) -> Vec<Clip> {
    let mut next = Vec::with_capacity(clips.len() + 1);
    for c in clips {
        if c.end() <= at {
            next.push(c.clone());
        } else if c.start >= at {
            next.push(Clip {
                start: c.start + len,
                ..c.clone()
            });
        } else {
            let (left, mut right) = split_clip(c, at);
            right.start = right.start + len;
            next.push(left);
            next.push(right);
        }
    }
    next
}

/// `track`'s clips with the one at `ix` replaced by `clip` and every clip
/// after it moved by `shift`.
pub(super) fn replace_shifting(track: &Track, ix: usize, clip: Clip, shift: Time) -> Vec<Clip> {
    let mut clips: Vec<Clip> = track.clips[..ix].to_vec();
    clips.push(clip);
    clips.extend(track.clips[ix + 1..].iter().map(|c| Clip {
        start: c.start + shift,
        ..c.clone()
    }));
    clips
}

/// Stretch the clip-relative times of `clip`'s keyframes, effect keyframes
/// and mask keyframes by `ratio`, for a clip whose length changes by that
/// factor (2.0 = twice as long). Speed keys are left to the caller, since
/// each retime changes their speeds its own way; generator and transition
/// keyframes keep their times.
pub(crate) fn stretch_keyframes(clip: &mut Clip, ratio: f64) {
    for k in &mut clip.keyframes {
        k.at = k.at.scale(ratio);
    }
    for e in &mut clip.effects {
        e.scale_keyframes(ratio);
    }
    for m in &mut clip.masks {
        m.scale_keyframes(ratio);
    }
}

/// Keyframes at or before clip time `t`, plus one at `t` for each property
/// that had keyframes after it, so the value at the cut does not jump.
fn keyframes_until(clip: &Clip, t: Time) -> Vec<Keyframe> {
    let mut keys: Vec<Keyframe> = clip
        .keyframes
        .iter()
        .filter(|k| k.at <= t)
        .cloned()
        .collect();
    for k in clip.keyframes.iter().filter(|k| k.at > t) {
        if !keys.iter().any(|x| x.property == k.property && x.at == t) {
            keys.push(Keyframe {
                property: k.property,
                at: t,
                value: clip.value_at(k.property, t),
                ease: k.ease,
            });
        }
    }
    keys.sort_by_key(|k| k.at);
    keys
}

/// Keyframes at or after clip time `t`, shifted to start at zero, plus one at
/// zero for each property that had keyframes before `t`.
fn keyframes_from(clip: &Clip, t: Time) -> Vec<Keyframe> {
    let mut keys: Vec<Keyframe> = clip
        .keyframes
        .iter()
        .filter(|k| k.at >= t)
        .map(|k| Keyframe {
            at: k.at - t,
            ..k.clone()
        })
        .collect();
    for k in clip.keyframes.iter().filter(|k| k.at < t) {
        if !keys
            .iter()
            .any(|x| x.property == k.property && x.at == Time::ZERO)
        {
            let ease = clip
                .keyframes
                .iter()
                .filter(|x| x.property == k.property && x.at < t)
                .max_by_key(|x| x.at)
                .map_or(k.ease, |x| x.ease);
            keys.push(Keyframe {
                property: k.property,
                at: Time::ZERO,
                value: clip.value_at(k.property, t),
                ease,
            });
        }
    }
    keys.sort_by_key(|k| k.at);
    keys
}
