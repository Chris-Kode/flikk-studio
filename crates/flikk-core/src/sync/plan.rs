//! Putting synced recordings in place: which clips make up a recording,
//! which is the reference, and the ops of [`Command::SyncClips`] and
//! [`Command::PlaceSynced`].
//!
//! Commands carry what was measured (offset and drift in source time), not
//! positions, so a proposal planned again after the user moved the
//! reference still lands in sync with it.
//!
//! [`Command::SyncClips`]: crate::command::Command::SyncClips
//! [`Command::PlaceSynced`]: crate::command::Command::PlaceSynced

use crate::command::Plan;
use crate::error::{CoreError, Result};
use crate::ids::{ClipId, MediaId, TrackId};
use crate::model::{Clip, ClipContent, MediaKind, Project, Timeline, Track, TrackKind};
use crate::ops::Op;
use crate::time::Time;
use serde::{Deserialize, Serialize};

/// A follower and where its sound sits against the reference's (see
/// [`super::Measured`]): its source time `offset + r · (1 + drift)` sounds
/// like the reference's source time `r`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SyncedClip {
    pub clip: ClipId,
    /// Seconds.
    pub offset: f64,
    #[serde(default)]
    pub drift: f64,
}

/// A pool item placed in sync with the reference, as [`SyncedClip`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SyncedMedia {
    pub media: MediaId,
    pub offset: f64,
    #[serde(default)]
    pub drift: f64,
}

/// One recording on the timeline: the clips that play one media item from
/// the same point (a video and its sound, landed as a pair).
#[derive(Clone, Debug, PartialEq)]
pub struct Recording {
    /// The picture's clip when there is one, else the sound's.
    pub clip: ClipId,
    pub clips: Vec<ClipId>,
    pub media: MediaId,
    /// The audio stream to hear, as `MediaService::audio` takes it.
    pub stream: u32,
    /// The media is sound only: an external recorder.
    pub sound_only: bool,
}

/// Clips that play `clip`'s media from the same point, on any track,
/// `clip` first.
pub fn linked(timeline: &Timeline, clip: &Clip) -> Vec<ClipId> {
    let mut out = vec![clip.id];
    let Some(key) = link_key(clip) else {
        return out;
    };
    for t in &timeline.tracks {
        for c in &t.clips {
            if c.id != clip.id && link_key(c) == Some(key) {
                out.push(c.id);
            }
        }
    }
    out
}

fn link_key(c: &Clip) -> Option<(MediaId, Time, Time, u64)> {
    match &c.content {
        ClipContent::Media {
            media, source_in, ..
        } => Some((*media, c.start, *source_in, c.speed.to_bits())),
        _ => None,
    }
}

/// The recordings among `selected`, the reference first: the one on the
/// lowest video track (the earliest there), else the first selected.
/// Clips without sound and repeats of one recording are left out.
pub fn recordings(project: &Project, selected: &[ClipId]) -> Vec<Recording> {
    let tl = &project.timeline;
    let mut out: Vec<(Recording, Option<(usize, Time)>)> = Vec::new();
    for id in selected {
        let Some((_, clip)) = tl.clip(*id) else {
            continue;
        };
        if out.iter().any(|(r, _)| r.clips.contains(id)) {
            continue;
        }
        let Some(media) = clip.media().and_then(|m| project.media(m)) else {
            continue;
        };
        if media.kind == MediaKind::Image || media.info.as_ref().is_some_and(|i| i.audio.is_empty())
        {
            continue;
        }
        let clips = linked(tl, clip);
        let on = |kinds: &[TrackKind]| {
            clips
                .iter()
                .copied()
                .find(|c| tl.clip(*c).is_some_and(|(t, _)| kinds.contains(&t.kind)))
        };
        let picture = on(&[TrackKind::Video, TrackKind::Overlay]);
        let sound = on(&[TrackKind::Audio]);
        let stream = sound
            .and_then(|c| tl.clip(c))
            .and_then(|(_, c)| match c.content {
                ClipContent::Media { audio_stream, .. } => audio_stream,
                _ => None,
            })
            .unwrap_or(0);
        let lowest_video = picture.and_then(|c| {
            let index = tl
                .tracks
                .iter()
                .position(|t| t.clips.iter().any(|x| x.id == c))?;
            let start = tl.clip(c)?.1.start;
            (tl.tracks[index].kind == TrackKind::Video).then_some((index, start))
        });
        out.push((
            Recording {
                clip: picture.unwrap_or(clip.id),
                clips,
                media: media.id,
                stream,
                sound_only: media.kind == MediaKind::Audio,
            },
            lowest_video,
        ));
    }
    // Tracks are listed top to bottom: the lowest video track has the
    // highest index.
    let reference = out
        .iter()
        .enumerate()
        .filter_map(|(i, (_, v))| v.map(|(track, start)| (i, track, start)))
        .max_by(|a, b| a.1.cmp(&b.1).then(b.2.cmp(&a.2)))
        .map_or(0, |(i, ..)| i);
    if reference != 0 {
        let r = out.remove(reference);
        out.insert(0, r);
    }
    out.into_iter().map(|(r, _)| r).collect()
}

/// The sound clips to disable so only the external recorders are heard:
/// the cameras' sound, when the group has a recorder.
pub fn camera_sound(project: &Project, recordings: &[Recording]) -> Vec<ClipId> {
    if !recordings.iter().any(|r| r.sound_only) {
        return Vec::new();
    }
    let tl = &project.timeline;
    recordings
        .iter()
        .filter(|r| !r.sound_only)
        .flat_map(|r| r.clips.iter().copied())
        .filter(|c| tl.clip(*c).is_some_and(|(t, _)| t.kind == TrackKind::Audio))
        .collect()
}

/// Where a follower goes: its start and speed against the reference clip
/// (reference source `r` plays at `start + (r - source_in) / speed`).
fn follower_timing(
    reference: (Time, Time, f64),
    follower_in: Time,
    offset: f64,
    drift: f64,
) -> Result<(f64, f64)> {
    let (start, source_in, speed) = reference;
    let factor = 1.0 + drift;
    if !(0.5..2.0).contains(&factor) || !offset.is_finite() {
        return Err(CoreError::Invalid("that sync is out of range".into()));
    }
    let follower_speed = speed * factor;
    let at = start.as_secs_f64()
        - source_in.as_secs_f64() / speed
        - (offset - follower_in.as_secs_f64()) / follower_speed;
    Ok((at, follower_speed))
}

/// `clip` moved to start at `at` (seconds) at `speed`, keeping the source
/// it shows; its head is trimmed where it would start before zero.
fn retimed(clip: &Clip, at: f64, speed: f64) -> Result<Clip> {
    let mut c = clip.clone();
    let span = clip.duration.as_secs_f64() * clip.speed;
    let mut duration = span / speed;
    let mut start = at;
    if start < 0.0 {
        let cut = -start;
        duration -= cut;
        if let ClipContent::Media { source_in, .. } = &mut c.content {
            *source_in = *source_in + Time::from_secs_f64(cut * speed);
        }
        start = 0.0;
    }
    if duration <= 0.0 {
        return Err(CoreError::Invalid(format!(
            "{} would land before the timeline's start",
            clip.id
        )));
    }
    c.start = Time::from_secs_f64(start);
    c.duration = Time::from_secs_f64(duration);
    c.speed = speed;
    Ok(c)
}

/// A ramped clip's sound keeps no one offset, and a drift fix (a constant
/// speed) would replace its ramp.
fn ramped(clip: &Clip) -> CoreError {
    CoreError::Invalid(format!(
        "{} has a speed ramp; sync it before ramping it",
        clip.id
    ))
}

/// Builds ops against a working copy, so each one is planned on the
/// project as the ones before it left it.
struct Builder {
    work: Project,
    ops: Vec<Op>,
}

impl Builder {
    fn new(project: &Project) -> Self {
        Self {
            work: project.clone(),
            ops: Vec::new(),
        }
    }

    fn push(&mut self, op: Op) -> Result<()> {
        op.apply(&mut self.work)?;
        self.ops.push(op);
        Ok(())
    }

    fn fits(&self, track: TrackId, clip: &Clip) -> bool {
        self.work.timeline.track(track).is_some_and(|t| {
            !t.locked
                && t.clips
                    .iter()
                    .all(|c| c.id == clip.id || !c.range().overlaps(&clip.range()))
        })
    }

    /// An unlocked track of `kind` with room for `clip`, else a new one.
    fn track_for(&mut self, kind: TrackKind, clip: &Clip) -> Result<TrackId> {
        let free = self
            .work
            .timeline
            .tracks
            .iter()
            .filter(|t| t.kind == kind)
            .map(|t| t.id)
            .find(|t| self.fits(*t, clip));
        if let Some(t) = free {
            return Ok(t);
        }
        let (index, name) = self.work.timeline.new_track_slot(kind);
        let track = Track::new(kind, name);
        let id = track.id;
        self.push(Op::InsertTrack { index, track })?;
        Ok(id)
    }

    /// Put `after` (a moved copy of a clip on `track`) in place: where it
    /// is when there is room, else on another track of its kind.
    fn relocate(&mut self, track: TrackId, after: Clip) -> Result<()> {
        let before = self
            .work
            .timeline
            .clip(after.id)
            .map(|(_, c)| c.clone())
            .ok_or(CoreError::ClipNotFound(after.id))?;
        if before == after {
            return Ok(());
        }
        if self.fits(track, &after) {
            return self.push(Op::UpdateClip {
                track,
                before: Box::new(before),
                after: Box::new(after),
            });
        }
        let kind = self
            .work
            .timeline
            .track(track)
            .map(|t| t.kind)
            .ok_or(CoreError::TrackNotFound(track))?;
        self.push(Op::RemoveClip {
            track,
            clip: Box::new(before),
        })?;
        let to = self.track_for(kind, &after)?;
        self.push(Op::InsertClip {
            track: to,
            clip: Box::new(after),
        })
    }

    fn place(&mut self, kind: TrackKind, clip: Clip) -> Result<()> {
        let to = self.track_for(kind, &clip)?;
        self.push(Op::InsertClip {
            track: to,
            clip: Box::new(clip),
        })
    }

    fn finish(self) -> Plan {
        Plan {
            ops: self.ops,
            warnings: Vec::new(),
        }
    }
}

/// [`Command::SyncClips`](crate::command::Command::SyncClips).
pub fn sync_clips(
    project: &Project,
    reference: ClipId,
    followers: &[SyncedClip],
    mute: &[ClipId],
) -> Result<Plan> {
    let tl = &project.timeline;
    let (_, r) = tl
        .clip(reference)
        .ok_or(CoreError::ClipNotFound(reference))?;
    let ClipContent::Media { source_in, .. } = r.content else {
        return Err(CoreError::Invalid(format!(
            "{reference} is not a media clip"
        )));
    };
    if r.is_ramped() {
        return Err(ramped(r));
    }
    let reference_at = (r.start, source_in, r.speed);
    let reference_clips = linked(tl, r);
    let mut b = Builder::new(project);
    let mut moved_ids = reference_clips.clone();
    for f in followers {
        let (_, clip) = tl.clip(f.clip).ok_or(CoreError::ClipNotFound(f.clip))?;
        if moved_ids.contains(&f.clip) {
            continue;
        }
        let ClipContent::Media { source_in, .. } = clip.content else {
            return Err(CoreError::Invalid(format!(
                "{} is not a media clip",
                f.clip
            )));
        };
        let (at, speed) = follower_timing(reference_at, source_in, f.offset, f.drift)?;
        let group = linked(tl, clip);
        if let Some(c) = group
            .iter()
            .filter_map(|id| tl.clip(*id))
            .map(|(_, c)| c)
            .find(|c| c.is_ramped())
        {
            return Err(ramped(c));
        }
        moved_ids.extend(&group);
        for id in group {
            let (track, c) = tl.clip(id).ok_or(CoreError::ClipNotFound(id))?;
            if track.locked {
                return Err(CoreError::Invalid(format!(
                    "track {} is locked",
                    track.name
                )));
            }
            let moved = retimed(c, at, speed)?;
            let track = track.id;
            // A clip moved earlier in this plan may have left its track.
            let now = b
                .work
                .timeline
                .tracks
                .iter()
                .find(|t| t.clips.iter().any(|x| x.id == id))
                .map_or(track, |t| t.id);
            b.relocate(now, moved)?;
        }
    }
    for id in mute {
        let Some((track, c)) = b.work.timeline.clip(*id) else {
            continue;
        };
        if !c.enabled || track.kind != TrackKind::Audio {
            continue;
        }
        let track = track.id;
        let before = c.clone();
        let after = Clip {
            enabled: false,
            ..before.clone()
        };
        b.push(Op::UpdateClip {
            track,
            before: Box::new(before),
            after: Box::new(after),
        })?;
    }
    Ok(b.finish())
}

/// [`Command::PlaceSynced`](crate::command::Command::PlaceSynced): the
/// reference from `at`, each follower where its sound meets it, each on a
/// free track of its kind (or a new one), whole; a video's sound beside it,
/// disabled when listed in `mute`. Moved later together when one would
/// start before zero.
pub fn place_synced(
    project: &Project,
    at: Time,
    reference: MediaId,
    followers: &[SyncedMedia],
    mute: &[MediaId],
) -> Result<Plan> {
    let mut items = vec![(reference, 0.0, 1.0)];
    for f in followers {
        if f.media == reference {
            continue;
        }
        let (start, speed) =
            follower_timing((Time::ZERO, Time::ZERO, 1.0), Time::ZERO, f.offset, f.drift)?;
        items.push((f.media, start, speed));
    }
    let earliest = items.iter().map(|i| i.1).fold(0.0, f64::min);
    let base = at.as_secs_f64() - earliest.min(0.0);
    let mut b = Builder::new(project);
    for (media, start, speed) in items {
        let item = project
            .media(media)
            .ok_or(CoreError::MediaNotFound(media))?;
        let length = item
            .info
            .as_ref()
            .map(|i| i.duration)
            .filter(|d| *d > Time::ZERO)
            .ok_or_else(|| CoreError::Invalid(format!("{media} has no length")))?;
        let has_sound = item.info.as_ref().is_some_and(|i| !i.audio.is_empty());
        let clip = |enabled: bool| {
            let mut c = Clip::new(
                Time::from_secs_f64(base + start),
                Time::from_secs_f64(length.as_secs_f64() / speed),
                ClipContent::Media {
                    media,
                    source_in: Time::ZERO,
                    audio_stream: None,
                },
            );
            c.speed = speed;
            c.enabled = enabled;
            c
        };
        match item.kind {
            MediaKind::Audio => b.place(TrackKind::Audio, clip(true))?,
            MediaKind::Video => {
                b.place(TrackKind::Video, clip(true))?;
                if has_sound {
                    b.place(TrackKind::Audio, clip(!mute.contains(&media)))?;
                }
            }
            MediaKind::Image => {
                return Err(CoreError::Invalid(format!("{media} is a picture")));
            }
        }
    }
    Ok(b.finish())
}
