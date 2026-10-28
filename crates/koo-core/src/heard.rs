//! The sounds and music heard on the timeline, from analysis in source
//! time, and the beats and onsets a cut can be moved onto. The agent's
//! sound script and its `align` option share this; the timeline can snap
//! the same way.

use crate::analysis::{Analysis, Levels, Music, Sound};
use crate::ids::{ClipId, MediaId, TrackId};
use crate::model::{Clip, Fingerprint, Project, Track, TrackKind};
use crate::time::{FLICKS_PER_SECOND, Time, TimeRange};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// A sound as it plays on the timeline.
#[derive(Clone, Debug, PartialEq)]
pub struct HeardSound {
    pub track: TrackId,
    pub clip: ClipId,
    /// Cut to the clip's edges and the range asked about.
    pub timeline: TimeRange,
    pub sound: Sound,
}

/// Music as it plays on the timeline, with its beats in timeline time.
#[derive(Clone, Debug, PartialEq)]
pub struct HeardMusic {
    pub track: TrackId,
    pub clip: ClipId,
    pub media: MediaId,
    pub timeline: TimeRange,
    /// Its beats and downbeats on the timeline, inside `timeline`.
    pub beats: Vec<Time>,
    pub downbeats: Vec<Time>,
    /// The music in source time (all of it, not only this part).
    pub music: Music,
}

/// Media clips whose sound is heard over `range`, with their analysis:
/// audio tracks first, muted ones left out, then the other tracks' clips.
fn heard_clips(
    project: &Project,
    range: TimeRange,
    analysis: impl Fn(&Fingerprint) -> Option<Arc<Analysis>>,
) -> Vec<(&Track, &Clip, MediaId, Arc<Analysis>)> {
    let tl = &project.timeline;
    let tracks = tl
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Audio && !t.mix.mute)
        .chain(tl.tracks.iter().filter(|t| t.kind == TrackKind::Video));
    let mut cache: HashMap<MediaId, Option<Arc<Analysis>>> = HashMap::new();
    let mut out = Vec::new();
    for track in tracks {
        for clip in track
            .clips
            .iter()
            .filter(|c| c.enabled && c.range().overlaps(&range))
        {
            let Some(media) = clip.media() else {
                continue;
            };
            let found = cache.entry(media).or_insert_with(|| {
                project
                    .media(media)
                    .and_then(|m| m.fingerprint.as_ref())
                    .and_then(&analysis)
            });
            if let Some(a) = found {
                out.push((track, clip, media, a.clone()));
            }
        }
    }
    out
}

/// Sounds heard under `range`, in timeline order; the same sound reached
/// through a linked clip at the same time is listed once.
pub fn sounds_in_range(
    project: &Project,
    range: TimeRange,
    analysis: impl Fn(&Fingerprint) -> Option<Arc<Analysis>>,
) -> Vec<HeardSound> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (track, clip, media, a) in heard_clips(project, range, analysis) {
        for sound in &a.sounds {
            let Some(timeline) = place(clip, sound.range, range) else {
                continue;
            };
            if seen.insert((
                media,
                sound.range.start,
                timeline.start,
                sound.label.clone(),
            )) {
                out.push(HeardSound {
                    track: track.id,
                    clip: clip.id,
                    timeline,
                    sound: sound.clone(),
                });
            }
        }
    }
    out.sort_by_key(|s| (s.timeline.start, s.timeline.end));
    out
}

/// Music heard under `range`, in timeline order, listed once like sounds.
pub fn music_in_range(
    project: &Project,
    range: TimeRange,
    analysis: impl Fn(&Fingerprint) -> Option<Arc<Analysis>>,
) -> Vec<HeardMusic> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (track, clip, media, a) in heard_clips(project, range, analysis) {
        for music in &a.music {
            let Some(timeline) = place(clip, music.range, range) else {
                continue;
            };
            if !seen.insert((media, music.range.start, timeline.start)) {
                continue;
            }
            let on_timeline = |times: &[Time]| {
                times
                    .iter()
                    .filter_map(|t| clip.to_timeline(*t))
                    .filter(|t| timeline.contains(*t))
                    .collect()
            };
            out.push(HeardMusic {
                track: track.id,
                clip: clip.id,
                media,
                timeline,
                beats: on_timeline(&music.beats),
                downbeats: on_timeline(&music.downbeats),
                music: music.clone(),
            });
        }
    }
    out.sort_by_key(|m| (m.timeline.start, m.timeline.end));
    out
}

/// Where the source range `source` of `clip` plays inside `range`.
fn place(clip: &Clip, source: TimeRange, range: TimeRange) -> Option<TimeRange> {
    clip.source_to_timeline_range(source)
        .intersect(&clip.range())?
        .intersect(&range)
}

/// What a cut can be moved onto.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Beat,
    /// The first beat of a bar.
    Downbeat,
    /// Where a sound starts: a sharp rise in the audio level.
    Onset,
}

impl Target {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "beat" => Some(Self::Beat),
            "downbeat" => Some(Self::Downbeat),
            "onset" => Some(Self::Onset),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Beat => "beat",
            Self::Downbeat => "downbeat",
            Self::Onset => "onset",
        }
    }
}

/// How far an onset is looked for, and the rise in level that makes one
/// (over the loudest of the 50 ms before it).
const ONSET_REACH: Time = Time(FLICKS_PER_SECOND * 15 / 100);
const ONSET_RISE_DB: f32 = 6.0;
const ONSET_BEFORE: usize = 5;

/// Timeline times of `target` near `t`, nearest first: beats within half
/// a beat, downbeats within half a bar, onsets within 150 ms.
pub fn cut_targets(
    project: &Project,
    t: Time,
    target: Target,
    analysis: impl Fn(&Fingerprint) -> Option<Arc<Analysis>>,
) -> Vec<Time> {
    let mut out: Vec<Time> = Vec::new();
    match target {
        Target::Beat | Target::Downbeat => {
            let wide = TimeRange::new(
                t - Time(FLICKS_PER_SECOND * 4),
                t + Time(FLICKS_PER_SECOND * 4),
            );
            for m in music_in_range(project, wide, &analysis) {
                let (times, gap) = if target == Target::Beat {
                    (
                        &m.beats,
                        m.music.bpm().map(|b| Time::from_secs_f64(60.0 / b as f64)),
                    )
                } else {
                    (&m.downbeats, m.music.bar())
                };
                let reach = Time(gap.unwrap_or(Time(FLICKS_PER_SECOND)).0 / 2);
                out.extend(times.iter().filter(|b| (b.0 - t.0).abs() <= reach.0));
            }
        }
        Target::Onset => {
            let near = TimeRange::new(t - ONSET_REACH, t + ONSET_REACH);
            for (_, clip, _, a) in heard_clips(project, near, &analysis) {
                if let Some(levels) = &a.levels {
                    out.extend(onsets(clip, levels, near));
                }
            }
        }
    }
    out.sort_by_key(|b| ((b.0 - t.0).abs(), b.0));
    out.dedup();
    out
}

/// Onsets of `clip`'s source inside timeline `near`: frames whose level
/// rises [`ONSET_RISE_DB`] over the loudest of the frames just before,
/// the steepest of each run.
fn onsets(clip: &Clip, levels: &Levels, near: TimeRange) -> Vec<Time> {
    let (Some(a), Some(b)) = (clip.to_source(near.start), clip.to_source(near.end)) else {
        return Vec::new();
    };
    let frame = |t: Time| (t.0.max(0) / Levels::FRAME.0) as usize;
    let (lo, hi) = (
        frame(a.min(b)).max(ONSET_BEFORE),
        frame(a.max(b)).min(levels.len()),
    );
    let rise = |i: usize| {
        let before = (i - ONSET_BEFORE..i)
            .map(|j| levels.db(j))
            .fold(f32::MIN, f32::max);
        levels.db(i) - before
    };
    let mut out = Vec::new();
    let mut i = lo;
    while i < hi {
        if rise(i) < ONSET_RISE_DB {
            i += 1;
            continue;
        }
        let mut best = i;
        while i < hi && rise(i) >= ONSET_RISE_DB {
            if rise(i) > rise(best) {
                best = i;
            }
            i += 1;
        }
        if let Some(t) = clip.to_timeline(Time(best as i64 * Levels::FRAME.0)) {
            out.push(t);
        }
    }
    out
}

/// A change in level this big, held [`CHANGE_SPAN`] either side, is a
/// change in the music (a drop, a breakdown).
const CHANGE_DB: f32 = 6.0;
const CHANGE_SPAN: Time = Time(FLICKS_PER_SECOND * 4);

/// Where `music` gets louder or softer, with the change in dB: at its
/// downbeats (every second without them), the biggest change within
/// [`CHANGE_SPAN`] of each other. `levels` are its source's.
pub fn level_changes(project: &Project, music: &HeardMusic, levels: &Levels) -> Vec<(Time, f32)> {
    let Some((_, clip)) = project.timeline.clip(music.clip) else {
        return Vec::new();
    };
    let span = music.timeline;
    let mean = |r: TimeRange| {
        let mut sum = 0.0;
        let mut n = 0;
        let mut t = r.start;
        while t < r.end {
            if let Some(db) = clip.to_source(t).and_then(|s| levels.at(s)) {
                sum += db;
                n += 1;
            }
            t = t + Levels::FRAME;
        }
        (n > 0).then(|| sum / n as f32)
    };
    let half = Time(CHANGE_SPAN.0 / 2);
    let candidates: Vec<Time> = if music.downbeats.len() >= 2 {
        music.downbeats.clone()
    } else {
        let mut out = Vec::new();
        let mut t = span.start;
        while t < span.end {
            out.push(t);
            t = t + Time(FLICKS_PER_SECOND);
        }
        out
    };
    let changes: Vec<(Time, f32)> = candidates
        .into_iter()
        .filter(|&c| c - span.start >= half && span.end - c >= half)
        .filter_map(|c| {
            let before = mean(TimeRange::new((c - CHANGE_SPAN).max(span.start), c))?;
            let after = mean(TimeRange::new(c, (c + CHANGE_SPAN).min(span.end)))?;
            Some((c, after - before))
        })
        .collect();
    changes
        .iter()
        .filter(|(c, d)| {
            d.abs() >= CHANGE_DB
                && changes
                    .iter()
                    .filter(|(o, _)| (o.0 - c.0).abs() <= CHANGE_SPAN.0)
                    .all(|(_, od)| od.abs() <= d.abs())
        })
        .copied()
        .collect()
}

/// Stretches of `range` at least `min` long where nothing heard is louder
/// than `below` dBFS (or nothing plays). Clips without measured levels
/// count as sound.
pub fn silences_in_range(
    project: &Project,
    range: TimeRange,
    analysis: impl Fn(&Fingerprint) -> Option<Arc<Analysis>>,
    below: f32,
    min: Time,
) -> Vec<TimeRange> {
    let tl = &project.timeline;
    let clips: Vec<(&Clip, Option<Arc<Analysis>>)> = tl
        .tracks
        .iter()
        .filter(|t| matches!(t.kind, TrackKind::Audio | TrackKind::Video))
        .filter(|t| !(t.kind == TrackKind::Audio && t.mix.mute))
        .flat_map(|t| t.clips.iter())
        .filter(|c| c.enabled && c.media().is_some() && c.range().overlaps(&range))
        .map(|c| {
            let a = c
                .media()
                .and_then(|m| project.media(m))
                .and_then(|m| m.fingerprint.as_ref())
                .and_then(&analysis);
            (c, a)
        })
        .collect();
    let quiet = |t: Time| {
        clips
            .iter()
            .filter(|(c, _)| c.range().contains(t))
            .all(|(c, a)| {
                let level = a
                    .as_ref()
                    .and_then(|a| a.levels.as_ref())
                    .zip(c.to_source(t))
                    .and_then(|(l, s)| l.at(s));
                level.is_some_and(|db| db < below)
            })
    };
    let mut out = Vec::new();
    let mut open: Option<Time> = None;
    let mut t = range.start;
    while t < range.end {
        match (open, quiet(t)) {
            (None, true) => open = Some(t),
            (Some(s), false) => {
                if t - s >= min {
                    out.push(TimeRange::new(s, t));
                }
                open = None;
            }
            _ => {}
        }
        t = t + Levels::FRAME;
    }
    if let Some(s) = open
        && range.end - s >= min
    {
        out.push(TimeRange::new(s, range.end));
    }
    out
}
