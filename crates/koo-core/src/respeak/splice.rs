//! [`Command::ReplaceSpeech`]: other words cut in where speech was.
//!
//! On the speech's track the range goes and the new recording plays from
//! its start, with a fade at both joins (they sit in pauses, so the fade
//! only keeps them from clicking); it keeps the gain and audio effects of
//! the clip it replaces. When the new words are shorter, the rest of the
//! timeline closes up as a ripple delete of the difference would (at the
//! end of the range, so the picture over the new words stays as it was).
//! When they are longer, everything after the range moves later on every
//! unlocked track: the picture over the range slows to fill it, other
//! audio leaves a gap, and titles and captions over the end of the range
//! last longer. The same source heard on another audio track (a second
//! microphone channel) is cut out of the range too, so the old words go.
//!
//! [`Command::ReplaceSpeech`]: crate::command::Command::ReplaceSpeech

use crate::command::Plan;
use crate::edit::{cut_out, diff_track, media_duration, split_clip};
use crate::error::{CoreError, Result};
use crate::ids::*;
use crate::model::*;
use crate::ops::Op;
use crate::time::{Time, TimeRange};

pub(crate) fn plan(
    project: &Project,
    range: TimeRange,
    track: TrackId,
    media: MediaId,
    fade: Time,
) -> Result<Plan> {
    let tl = &project.timeline;
    let target = tl.track(track).ok_or(CoreError::TrackNotFound(track))?;
    if target.locked {
        return Err(CoreError::Invalid(format!(
            "track {} is locked",
            target.name
        )));
    }
    if target.kind != TrackKind::Audio {
        return Err(CoreError::Invalid(
            "speech is replaced on audio tracks".into(),
        ));
    }
    if range.is_empty() || range.start < Time::ZERO {
        return Err(CoreError::Invalid("empty range".into()));
    }
    let item = project
        .media(media)
        .ok_or(CoreError::MediaNotFound(media))?;
    if item.kind == MediaKind::Image || item.info.as_ref().is_some_and(|i| i.audio.is_empty()) {
        return Err(CoreError::Invalid("the new words have no sound".into()));
    }
    let len = media_duration(project, media)
        .ok_or_else(|| CoreError::Invalid("the length of the new words is unknown".into()))?;
    let replaced = target
        .clips
        .iter()
        .find(|c| c.range().contains(range.start))
        .ok_or_else(|| CoreError::Invalid("no speech there to replace".into()))?;
    let old = replaced.media();
    let delta = len - range.duration();

    let mut ops: Vec<Op> = Vec::new();

    // The speech's own track.
    let mut next = Vec::with_capacity(target.clips.len() + 2);
    for c in cut_out(&target.clips, range) {
        if c.end() <= range.start {
            let mut c = c;
            if c.end() == range.start && c.media().is_some() {
                c.audio.fade_out = fade.min(c.duration);
            }
            next.push(c);
        } else {
            let mut c = Clip {
                start: c.start + delta,
                ..c
            };
            if c.start == range.start + len && c.media().is_some() {
                c.audio.fade_in = fade.min(c.duration);
            }
            next.push(c);
        }
    }
    let stream = item
        .info
        .as_ref()
        .and_then(|i| i.audio.first())
        .map(|a| a.index);
    let mut said = Clip::new(
        range.start,
        len,
        ClipContent::Media {
            media,
            source_in: Time::ZERO,
            audio_stream: stream,
        },
    );
    said.audio = ClipAudio {
        gain_db: replaced.audio.gain_db,
        fade_in: fade.min(len),
        fade_out: fade.min(len),
        effects: replaced.audio.effects.clone(),
    };
    next.push(said);
    ops.extend(diff_track(target, next));

    // Every other unlocked track.
    for t in tl.tracks.iter().filter(|t| t.id != track && !t.locked) {
        let next = if delta < Time::ZERO {
            shorten(t, range, len, old, fade)
        } else {
            lengthen(t, range, delta, old, fade)
        };
        ops.extend(diff_track(t, next));
    }

    // Markers move with what they mark.
    for m in &tl.markers {
        let r = m.range;
        let after = if delta < Time::ZERO {
            // As a ripple delete of the range's end.
            let gone = TimeRange::new(range.start + len, range.end);
            let lost = gone.duration();
            if r.start >= gone.end {
                Some(r.shift(-lost))
            } else if r.start < gone.start {
                let end = if r.end <= gone.start {
                    r.end
                } else if r.end <= gone.end {
                    gone.start
                } else {
                    r.end - lost
                };
                Some(TimeRange::new(r.start, end))
            } else if r.end > gone.end {
                Some(TimeRange::new(gone.start, r.end - lost))
            } else {
                None
            }
        } else if r.start >= range.end {
            Some(r.shift(delta))
        } else if r.end > range.end || (r.end == range.end && r.start < range.end) {
            Some(TimeRange::new(r.start, r.end + delta))
        } else {
            Some(r)
        };
        match after {
            Some(r) if r == m.range => {}
            Some(r) => ops.push(Op::UpdateMarker {
                before: m.clone(),
                after: Marker {
                    range: r,
                    ..m.clone()
                },
            }),
            None => ops.push(Op::RemoveMarker { marker: m.clone() }),
        }
    }
    Ok(Plan {
        ops,
        warnings: Vec::new(),
    })
}

/// Whether `c` on `t` plays the old speech's source, whose words go.
fn same_sound(t: &Track, c: &Clip, old: Option<MediaId>) -> bool {
    t.kind == TrackKind::Audio && old.is_some() && c.media() == old
}

/// `c` in three: before `a`, from `a` to `b`, after `b` (each when it has
/// any). The first piece keeps the id.
fn thirds(c: &Clip, a: Time, b: Time) -> (Option<Clip>, Option<Clip>, Option<Clip>) {
    let mut rest = c.clone();
    let mut left = None;
    if rest.start < a && a < rest.end() {
        let (l, r) = split_clip(&rest, a);
        left = Some(l);
        rest = r;
    } else if rest.end() <= a {
        return (Some(rest), None, None);
    }
    if rest.start >= b {
        return (left, None, Some(rest));
    }
    if b < rest.end() {
        let (m, r) = split_clip(&rest, b);
        return (left, Some(m), Some(r));
    }
    (left, Some(rest), None)
}

fn faded(mut c: Clip, kind: TrackKind, fade: Time, out: bool) -> Clip {
    if kind == TrackKind::Audio && c.media().is_some() && fade > Time::ZERO {
        if out {
            c.audio.fade_out = fade.min(c.duration);
        } else {
            c.audio.fade_in = fade.min(c.duration);
        }
    }
    c
}

/// Another track when the new words are shorter, `len` of the range's:
/// the old speech heard here goes with the range, everything else as a
/// ripple delete of the range's end.
fn shorten(t: &Track, range: TimeRange, len: Time, old: Option<MediaId>, fade: Time) -> Vec<Clip> {
    let lost = range.duration() - len;
    let mut next = Vec::with_capacity(t.clips.len() + 1);
    for c in &t.clips {
        let gone = if same_sound(t, c, old) {
            range
        } else {
            TimeRange::new(range.start + len, range.end)
        };
        if c.end() <= gone.start {
            next.push(c.clone());
            continue;
        }
        if c.start >= gone.end {
            next.push(Clip {
                start: c.start - lost,
                ..c.clone()
            });
            continue;
        }
        let (left, _, right) = thirds(c, gone.start, gone.end);
        if let Some(l) = left {
            next.push(faded(l, t.kind, fade, true));
        }
        if let Some(r) = right {
            let mut r = faded(r, t.kind, fade, false);
            r.start = r.start - lost;
            next.push(r);
        }
    }
    next
}

/// Another track when the new words are `delta` longer.
fn lengthen(
    t: &Track,
    range: TimeRange,
    delta: Time,
    old: Option<MediaId>,
    fade: Time,
) -> Vec<Clip> {
    let picture = matches!(t.kind, TrackKind::Video | TrackKind::Overlay);
    let mut next = Vec::with_capacity(t.clips.len() + 2);
    for c in &t.clips {
        if c.end() <= range.start {
            next.push(c.clone());
            continue;
        }
        if c.start >= range.end {
            next.push(Clip {
                start: c.start + delta,
                ..c.clone()
            });
            continue;
        }
        if c.media().is_none() {
            // Titles, captions, generators: last over the longer range.
            let mut c = c.clone();
            if c.end() > range.end || (c.end() == range.end && c.start < range.end) {
                c.duration = c.duration + delta;
            }
            next.push(c);
            continue;
        }
        let (left, mid, right) = thirds(c, range.start, range.end);
        let same = same_sound(t, c, old);
        if let Some(l) = left {
            next.push(if same {
                faded(l, t.kind, fade, true)
            } else {
                l
            });
        }
        if let Some(m) = mid {
            if picture {
                next.push(held(m, range, delta));
            } else if !same {
                next.push(m);
            }
        }
        if let Some(mut r) = right {
            r.start = r.start + delta;
            if same {
                r = faded(r, t.kind, fade, false);
            }
            next.push(r);
        }
    }
    next
}

/// The picture of `m`, inside `range`, slowed so `range` lasts `delta`
/// longer.
fn held(mut m: Clip, range: TimeRange, delta: Time) -> Clip {
    let span = range.duration().0 as f64;
    let ratio = (span + delta.0 as f64) / span;
    m.start = range.start + (m.start - range.start).scale(ratio);
    let duration = m.duration.scale(ratio);
    m.speed = (m.speed / ratio).max(crate::speed::MIN_SPEED);
    m.duration = duration;
    // A ramp keeps its shape, slower by the same ratio.
    for k in &mut m.retime.keys {
        k.at = k.at.scale(ratio);
        k.speed = (k.speed / ratio).max(crate::speed::MIN_SPEED);
    }
    for k in &mut m.keyframes {
        k.at = k.at.scale(ratio);
    }
    for e in &mut m.effects {
        e.scale_keyframes(ratio);
    }
    for mask in &mut m.masks {
        mask.scale_keyframes(ratio);
    }
    m
}
