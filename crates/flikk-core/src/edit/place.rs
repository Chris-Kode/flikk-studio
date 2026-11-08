//! Where clips may go, and planners that put clips on a track.

use super::bounds::media_duration;
use super::diff::diff_track;
use super::effects::{coerced, fx_asset};
use super::ops;
use super::retime::{speed_error, valid_speed};
use super::slice::{cut_out, open_gap};
use super::target::{editable_track, find_clip};
use crate::command::{InsertMode, PlacedClip, Plan};
use crate::error::{CoreError, Result};
use crate::fx::{FxInstance, FxKind};
use crate::ids::*;
use crate::model::*;
use crate::ops::Op;
use crate::time::{Time, TimeRange};

pub(super) fn move_clip(
    project: &Project,
    id: ClipId,
    at: Time,
    to: Option<TrackId>,
) -> Result<Plan> {
    let tl = &project.timeline;
    let (from, clip) = find_clip(tl, id)?;
    let to = to.unwrap_or(from.id);
    editable_track(tl, from.id)?;
    let target = editable_track(tl, to)?;
    if to != from.id && !fits_track(project, &clip.content, target.kind) {
        return Err(CoreError::Invalid(format!(
            "that clip does not go on {} ({:?})",
            target.name, target.kind
        )));
    }
    let moved = Clip {
        start: at,
        ..clip.clone()
    };
    if at < Time::ZERO
        || target
            .clips
            .iter()
            .any(|c| c.id != id && c.range().overlaps(&moved.range()))
    {
        return Err(CoreError::Invalid(
            "the clip would overlap another clip".into(),
        ));
    }
    Ok(ops(if to == from.id {
        vec![Op::UpdateClip {
            track: to,
            before: Box::new(clip.clone()),
            after: Box::new(moved),
        }]
    } else {
        vec![
            Op::RemoveClip {
                track: from.id,
                clip: Box::new(clip.clone()),
            },
            Op::InsertClip {
                track: to,
                clip: Box::new(moved),
            },
        ]
    }))
}

pub(super) fn insert_clip(
    project: &Project,
    media: MediaId,
    source: TimeRange,
    track: TrackId,
    at: Time,
    mode: InsertMode,
) -> Result<Plan> {
    let tl = &project.timeline;
    let item = project
        .media(media)
        .ok_or(CoreError::MediaNotFound(media))?;
    let target = editable_track(tl, track)?;
    let content = ClipContent::Media {
        media,
        // Holding the source to the media's length only moves its end.
        source_in: source.start,
        audio_stream: (target.kind == TrackKind::Audio)
            .then(|| item.first_audio_stream())
            .flatten(),
    };
    if !fits_track(project, &content, target.kind) {
        return Err(CoreError::Invalid(format!(
            "{:?} media does not go on a {:?} track",
            item.kind, target.kind
        )));
    }
    if at < Time::ZERO || source.start < Time::ZERO {
        return Err(CoreError::Invalid("negative time".into()));
    }
    let source = match media_duration(project, media) {
        Some(d) => TimeRange::new(source.start, source.end.min(d)),
        None => source,
    };
    if source.is_empty() {
        return Err(CoreError::Invalid(
            "the source range is empty or outside the media".into(),
        ));
    }
    let len = source.duration();
    let clip = Clip::new(at, len, content);
    let mut next = match mode {
        InsertMode::Overwrite => cut_out(&target.clips, TimeRange::new(at, at + len)),
        InsertMode::Insert => open_gap(&target.clips, at, len),
    };
    next.push(clip);
    Ok(ops(diff_track(target, next)))
}

/// Songs back to back from `at`, each for its full duration, replacing
/// whatever the track has in that span (PRD A4).
pub(super) fn set_playlist(
    project: &Project,
    track: TrackId,
    at: Time,
    songs: &[MediaId],
) -> Result<Plan> {
    let tl = &project.timeline;
    let target = editable_track(tl, track)?;
    if target.kind != TrackKind::Audio {
        return Err(CoreError::Invalid("playlists go on audio tracks".into()));
    }
    if songs.is_empty() || at < Time::ZERO {
        return Err(CoreError::Invalid("no songs, or a negative start".into()));
    }
    let mut cursor = at;
    let mut clips = Vec::with_capacity(songs.len());
    for &media in songs {
        let item = project
            .media(media)
            .ok_or(CoreError::MediaNotFound(media))?;
        let duration = media_duration(project, media).ok_or_else(|| {
            CoreError::Invalid(format!("{} has no known duration", item.path.display()))
        })?;
        clips.push(Clip::new(
            cursor,
            duration,
            ClipContent::Media {
                media,
                source_in: Time::ZERO,
                audio_stream: item.first_audio_stream(),
            },
        ));
        cursor = cursor + duration;
    }
    let mut next = cut_out(&target.clips, TimeRange::new(at, cursor));
    next.extend(clips);
    Ok(ops(diff_track(target, next)))
}

/// Copies of clips on their target tracks (paste, duplicate).
pub(super) fn place_clips(
    project: &Project,
    items: &[PlacedClip],
    mode: InsertMode,
) -> Result<Plan> {
    let tl = &project.timeline;
    if items.is_empty() {
        return Err(CoreError::Invalid("nothing to place".into()));
    }
    let mut groups: Vec<(TrackId, Vec<Clip>)> = Vec::new();
    for item in items {
        let track = editable_track(tl, item.track)?;
        if !fits_track(project, &item.clip.content, track.kind) {
            return Err(CoreError::Invalid(format!(
                "that clip does not go on {} ({:?})",
                track.name, track.kind
            )));
        }
        if item.clip.start < Time::ZERO || item.clip.duration <= Time::ZERO {
            return Err(CoreError::Invalid("negative start or empty clip".into()));
        }
        let clip = placed_clip(project, &item.clip)?;
        match groups.iter_mut().find(|(t, _)| *t == item.track) {
            Some((_, clips)) => clips.push(clip),
            None => groups.push((item.track, vec![clip])),
        }
    }
    let mut out = Vec::new();
    for (track_id, mut placed) in groups {
        placed.sort_by_key(|c| c.start);
        if placed.windows(2).any(|w| w[0].end() > w[1].start) {
            return Err(CoreError::Invalid("placed clips overlap each other".into()));
        }
        let track = tl
            .track(track_id)
            .ok_or(CoreError::TrackNotFound(track_id))?;
        let first = placed[0].start;
        let span = placed.iter().map(Clip::end).max().unwrap_or(first) - first;
        let mut next = match mode {
            InsertMode::Overwrite => placed
                .iter()
                .fold(track.clips.clone(), |clips, c| cut_out(&clips, c.range())),
            InsertMode::Insert => open_gap(&track.clips, first, span),
        };
        next.extend(placed);
        out.extend(diff_track(track, next));
    }
    Ok(ops(out))
}

/// A copy of a caller's clip for [`place_clips`], new ids and all, held to
/// what the other planners keep: a speed and ramp in range, fades inside
/// the clip, keyframes in order, and programs of the right kind.
fn placed_clip(project: &Project, clip: &Clip) -> Result<Clip> {
    if !valid_speed(clip.speed) || !clip.retime.keys.iter().all(|k| valid_speed(k.speed)) {
        return Err(speed_error());
    }
    if !clip.keyframes.iter().all(|k| k.value.is_finite()) {
        return Err(CoreError::Invalid("invalid keyframe value".into()));
    }
    let mut c = clip.clone();
    c.id = ClipId::new();
    if c.media().is_some() {
        c.retime.keys.sort_by_key(|k| k.at);
        c.settle_ramp();
    } else {
        c.retime.keys.clear();
    }
    c.keyframes.sort_by_key(|k| k.at);
    c.audio.fade_in = c.audio.fade_in.clamp(Time::ZERO, c.duration);
    c.audio.fade_out = c.audio.fade_out.clamp(Time::ZERO, c.duration);
    // Each copy owns its effect instances.
    let instance = |fx: &FxInstance, kind: FxKind| -> Result<FxInstance> {
        let mut out = coerced(fx, fx_asset(project, &fx.fx, kind)?);
        out.id = EffectId::new();
        out.keyframes.sort_by_key(|k| k.at);
        Ok(out)
    };
    if let ClipContent::Generator { fx } = &mut c.content {
        *fx = instance(fx, FxKind::Generator)?;
    }
    c.effects = c
        .effects
        .iter()
        .map(|e| instance(e, FxKind::Effect))
        .collect::<Result<_>>()?;
    let caption = matches!(c.content, ClipContent::Caption { .. });
    for t in [&mut c.transition_in, &mut c.transition_out]
        .into_iter()
        .flatten()
    {
        if caption {
            return Err(CoreError::Invalid("captions take no transitions".into()));
        }
        if t.duration <= Time::ZERO {
            return Err(CoreError::Invalid("a transition needs a duration".into()));
        }
        t.fx = instance(&t.fx, FxKind::Transition)?;
    }
    for m in &mut c.masks {
        m.keyframes.sort_by_key(|k| k.at);
    }
    Ok(c)
}

/// Whether `content` may sit on a `kind` track. [`Project::normalize`]
/// holds files read from disk to the same rule.
pub(crate) fn fits_track(project: &Project, content: &ClipContent, kind: TrackKind) -> bool {
    match content {
        ClipContent::Media { media, .. } => {
            let Some(item) = project.media(*media) else {
                return false;
            };
            match kind {
                TrackKind::Audio => {
                    item.kind != MediaKind::Image
                        && item.info.as_ref().is_none_or(|i| !i.audio.is_empty())
                }
                TrackKind::Video | TrackKind::Overlay => item.kind != MediaKind::Audio,
                TrackKind::Title | TrackKind::Caption => false,
            }
        }
        ClipContent::Caption { .. } => kind == TrackKind::Caption,
        ClipContent::Title { .. } => kind == TrackKind::Title,
        ClipContent::Overlay { .. } => matches!(kind, TrackKind::Overlay | TrackKind::Video),
        ClipContent::Generator { .. } => matches!(
            kind,
            TrackKind::Video | TrackKind::Overlay | TrackKind::Title
        ),
    }
}
