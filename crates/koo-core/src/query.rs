//! Read-only questions about a project: keyframe values, what plays at a
//! time, collage sample points, transcript ↔ timeline mapping, chat
//! references and the style rules in force. Pure functions over the model;
//! the viewer, render, agent tools and panels all share them.

use crate::analysis::{Analysis, Word};
use crate::ids::*;
use crate::memory::{RuleScope, RuleStatus, StyleRule};
use crate::model::*;
use crate::services::SamplePoint;
use crate::time::{Time, TimeRange};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

// ---------------------------------------------------------------- keyframes

impl Ease {
    /// Progress `u` in 0..1 between two keyframes → eased 0..1.
    pub fn apply(self, u: f32) -> f32 {
        let u = u.clamp(0.0, 1.0);
        match self {
            Ease::Linear => u,
            Ease::EaseIn => u * u * u,
            Ease::EaseOut => 1.0 - (1.0 - u).powi(3),
            Ease::EaseInOut if u < 0.5 => 4.0 * u * u * u,
            Ease::EaseInOut => 1.0 - (-2.0 * u + 2.0).powi(3) / 2.0,
            Ease::Hold => 0.0,
        }
    }
}

impl Transform {
    /// [`Property::Volume`] is not a transform property and reads 0.
    pub fn get(&self, property: Property) -> f32 {
        match property {
            Property::PositionX => self.position[0],
            Property::PositionY => self.position[1],
            Property::Scale => self.scale,
            Property::Rotation => self.rotation_deg,
            Property::Opacity => self.opacity,
            Property::CropLeft => self.crop[0],
            Property::CropTop => self.crop[1],
            Property::CropRight => self.crop[2],
            Property::CropBottom => self.crop[3],
            Property::ScaleX => self.stretch[0],
            Property::ScaleY => self.stretch[1],
            Property::SkewX => self.skew_deg[0],
            Property::SkewY => self.skew_deg[1],
            Property::AnchorX => self.anchor[0],
            Property::AnchorY => self.anchor[1],
            Property::Volume => 0.0,
            p => match pin_index(p) {
                Some((corner, axis)) => self.corner_pin[corner][axis],
                None => 0.0,
            },
        }
    }

    /// Setting [`Property::Volume`] does nothing.
    pub fn set(&mut self, property: Property, value: f32) {
        match property {
            Property::PositionX => self.position[0] = value,
            Property::PositionY => self.position[1] = value,
            Property::Scale => self.scale = value,
            Property::Rotation => self.rotation_deg = value,
            Property::Opacity => self.opacity = value,
            Property::CropLeft => self.crop[0] = value,
            Property::CropTop => self.crop[1] = value,
            Property::CropRight => self.crop[2] = value,
            Property::CropBottom => self.crop[3] = value,
            Property::ScaleX => self.stretch[0] = value,
            Property::ScaleY => self.stretch[1] = value,
            Property::SkewX => self.skew_deg[0] = value,
            Property::SkewY => self.skew_deg[1] = value,
            Property::AnchorX => self.anchor[0] = value,
            Property::AnchorY => self.anchor[1] = value,
            Property::Volume => {}
            p => {
                if let Some((corner, axis)) = pin_index(p) {
                    self.corner_pin[corner][axis] = value;
                }
            }
        }
    }
}

/// The corner and axis of a [`Property::PIN`] property.
fn pin_index(p: Property) -> Option<(usize, usize)> {
    let i = Property::PIN.iter().position(|q| *q == p)?;
    Some((i / 2, i % 2))
}

impl Clip {
    /// The un-keyframed value of `property`: the [`Transform`] field, or the
    /// clip gain for [`Property::Volume`].
    pub fn base_value(&self, property: Property) -> f32 {
        match property {
            Property::Volume => self.audio.gain_db,
            p => self.transform.get(p),
        }
    }

    /// Value of `property` at clip-relative time `t`. Without keyframes for
    /// it, [`Clip::base_value`]; before the first and after the
    /// last keyframe, that keyframe's value; in between, eased by the earlier
    /// keyframe's [`Ease`].
    pub fn value_at(&self, property: Property, t: Time) -> f32 {
        let (mut prev, mut next): (Option<&Keyframe>, Option<&Keyframe>) = (None, None);
        for k in self.keyframes.iter().filter(|k| k.property == property) {
            if k.at <= t {
                if prev.is_none_or(|p| k.at >= p.at) {
                    prev = Some(k);
                }
            } else if next.is_none_or(|n| k.at < n.at) {
                next = Some(k);
            }
        }
        match (prev, next) {
            (None, None) => self.base_value(property),
            (Some(k), None) | (None, Some(k)) => k.value,
            (Some(a), Some(b)) => {
                let u = (t - a.at).0 as f64 / (b.at - a.at).0 as f64;
                a.value + (b.value - a.value) * a.ease.apply(u as f32)
            }
        }
    }

    /// The full transform at clip-relative time `t`.
    pub fn transform_at(&self, t: Time) -> Transform {
        let mut out = self.transform.clone();
        for p in Property::TRANSFORM {
            out.set(p, self.value_at(p, t));
        }
        out
    }

    /// Where a source range lands on the timeline through this clip's speed,
    /// unclamped (it may lie outside the clip).
    pub fn source_to_timeline_range(&self, source: TimeRange) -> TimeRange {
        let source_in = match &self.content {
            ClipContent::Media { source_in, .. } => *source_in,
            _ => Time::ZERO,
        };
        let inv = 1.0 / self.speed;
        TimeRange::new(
            self.start + (source.start - source_in).scale(inv),
            self.start + (source.end - source_in).scale(inv),
        )
    }

    /// Source range this clip plays, for media clips (`duration × speed`).
    pub fn source_range(&self) -> Option<TimeRange> {
        match &self.content {
            ClipContent::Media { source_in, .. } => Some(TimeRange::new(
                *source_in,
                *source_in + self.duration.scale(self.speed),
            )),
            _ => None,
        }
    }
}

// ------------------------------------------------------------ what plays at t

/// Clips drawn at timeline time `t`, topmost first: every shown (not muted)
/// non-audio track's enabled clip whose opacity there is above zero, as
/// rendered.
pub fn visible_clips_at(tl: &Timeline, t: Time) -> Vec<(&Track, &Clip)> {
    tl.tracks
        .iter()
        .filter(|tr| tr.kind != TrackKind::Audio && !tr.mix.mute)
        .filter_map(|tr| tr.clip_at(t).map(|c| (tr, c)))
        .filter(|(_, c)| c.enabled)
        .filter(|(_, c)| c.value_at(Property::Opacity, t - c.start) > 0.0)
        .collect()
}

/// The topmost visible media clip on a video or overlay track at `t`.
pub fn topmost_video_clip(tl: &Timeline, t: Time) -> Option<(&Track, &Clip)> {
    visible_clips_at(tl, t).into_iter().find(|(tr, c)| {
        matches!(tr.kind, TrackKind::Video | TrackKind::Overlay) && c.media().is_some()
    })
}

/// Media clips heard at `t`: audio tracks that are not muted and, if any
/// track is soloed, only soloed ones.
pub fn audible_clips_at(tl: &Timeline, t: Time) -> Vec<(&Track, &Clip)> {
    let audio = || tl.tracks.iter().filter(|tr| tr.kind == TrackKind::Audio);
    let solo = audio().any(|tr| tr.mix.solo);
    audio()
        .filter(|tr| !tr.mix.mute && (!solo || tr.mix.solo))
        .filter_map(|tr| tr.clip_at(t).map(|c| (tr, c)))
        .filter(|(_, c)| c.enabled && c.media().is_some())
        .collect()
}

/// Most tiles a collage holds (PRD V2).
pub const MAX_SAMPLES: usize = 30;

/// Up to `n` (at most [`MAX_SAMPLES`]) frames spread evenly over `range`,
/// each at the centre of its slice and snapped to a sequence frame, resolved
/// through the topmost video clip to a source frame (PRD V3). Times with no
/// video are skipped.
pub fn sample_points(project: &Project, range: TimeRange, n: usize) -> Vec<SamplePoint> {
    let n = n.min(MAX_SAMPLES);
    if n == 0 || range.is_empty() {
        return Vec::new();
    }
    let tl = &project.timeline;
    let len = range.duration().0 as i128;
    let mut out: Vec<SamplePoint> = Vec::with_capacity(n);
    for i in 0..n as i128 {
        let t = range.start + Time((len * (2 * i + 1) / (2 * n as i128)) as i64);
        let t = t.floor_to_frame(tl.settings.rate).max(range.start);
        if out.last().is_some_and(|p| p.timeline == t) {
            continue;
        }
        let Some((_, clip)) = topmost_video_clip(tl, t) else {
            continue;
        };
        let (Some(media), Some(source)) = (clip.media(), clip.to_source(t)) else {
            continue;
        };
        let Some(item) = project.media(media) else {
            continue;
        };
        out.push(SamplePoint {
            timeline: t,
            media,
            path: item.path.clone(),
            fingerprint: item.fingerprint.clone(),
            source,
        });
    }
    out
}

// ------------------------------------------------------------- transcripts

/// A transcript word as it plays on the timeline.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TimelineWord {
    pub track: TrackId,
    pub clip: ClipId,
    pub media: MediaId,
    /// Where the word plays, cut to the clip's edges.
    pub timeline: TimeRange,
    /// The whole word in source time.
    pub source: TimeRange,
    pub word: Word,
}

/// Words heard under timeline `range`, in timeline order. Audio tracks are
/// read first; the same source word reached through a linked video clip at
/// the same time is listed once, and a source with clips on audio tracks
/// is read there only.
pub fn words_in_range(
    project: &Project,
    range: TimeRange,
    analysis: impl Fn(&Fingerprint) -> Option<Arc<Analysis>>,
) -> Vec<TimelineWord> {
    let tl = &project.timeline;
    let tracks = tl
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Audio)
        .chain(tl.tracks.iter().filter(|t| t.kind != TrackKind::Audio));
    let mut cache: HashMap<MediaId, Option<Arc<Analysis>>> = HashMap::new();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    // A source whose sound is laid on audio tracks is heard there only:
    // its picture alone says nothing (speech replaced under it, say).
    let heard: HashSet<MediaId> = tl
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Audio)
        .flat_map(|t| t.clips.iter().filter_map(Clip::media))
        .collect();
    for track in tracks {
        for clip in track.clips.iter().filter(|c| c.range().overlaps(&range)) {
            let (Some(media), Some(source)) = (clip.media(), clip.source_range()) else {
                continue;
            };
            if track.kind != TrackKind::Audio && heard.contains(&media) {
                continue;
            }
            let found = cache.entry(media).or_insert_with(|| {
                project
                    .media(media)
                    .and_then(|m| m.fingerprint.as_ref())
                    .and_then(&analysis)
            });
            let Some(transcript) = found.as_ref().and_then(|a| a.transcript.as_ref()) else {
                continue;
            };
            for word in transcript.words_in(source) {
                let Some(timeline) = clip
                    .source_to_timeline_range(word.range)
                    .intersect(&clip.range())
                    .filter(|r| r.overlaps(&range))
                else {
                    continue;
                };
                if seen.insert((media, word.range.start, timeline.start)) {
                    out.push(TimelineWord {
                        track: track.id,
                        clip: clip.id,
                        media,
                        timeline,
                        source: word.range,
                        word: word.clone(),
                    });
                }
            }
        }
    }
    out.sort_by_key(|w| (w.timeline.start, w.timeline.end));
    out
}

/// Where part of a source plays on the timeline.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceMapping {
    pub track: TrackId,
    pub clip: ClipId,
    pub timeline: TimeRange,
    pub source: TimeRange,
}

/// Every place the source range `source` of `media` is used, in timeline
/// order, one entry per clip.
pub fn source_to_timeline(
    project: &Project,
    media: MediaId,
    source: TimeRange,
) -> Vec<SourceMapping> {
    let mut out: Vec<SourceMapping> = project
        .timeline
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter().map(move |c| (t, c)))
        .filter(|(_, c)| c.media() == Some(media))
        .filter_map(|(t, c)| {
            let used = c.source_range()?;
            let s = used.intersect(&source)?;
            Some(SourceMapping {
                track: t.id,
                clip: c.id,
                timeline: c.source_to_timeline_range(s),
                source: s,
            })
        })
        .collect();
    out.sort_by_key(|m| (m.timeline.start, m.timeline.end));
    out
}

/// Timeline ranges for a selection of source ranges of `media` (e.g. words
/// picked in the transcript, PRD E3), merged and sorted. Delete them from the
/// last to the first so earlier ranges stay valid.
pub fn selection_to_timeline(
    project: &Project,
    media: MediaId,
    selection: &[TimeRange],
) -> Vec<TimeRange> {
    merge_ranges(
        selection
            .iter()
            .flat_map(|s| source_to_timeline(project, media, *s))
            .map(|m| m.timeline)
            .collect(),
    )
}

/// Sort and merge overlapping or touching ranges; empty ones are dropped.
pub fn merge_ranges(mut ranges: Vec<TimeRange>) -> Vec<TimeRange> {
    ranges.retain(|r| !r.is_empty());
    ranges.sort_by_key(|r| r.start);
    let mut out: Vec<TimeRange> = Vec::with_capacity(ranges.len());
    for r in ranges {
        match out.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}

// --------------------------------------------------------- chat references

/// Something a chat message points at (PRD AG3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "ref", rename_all = "snake_case")]
pub enum Reference {
    /// "1:01–1:04", "1:01-1:04", "from 1:01 to 1:04".
    Range { range: TimeRange },
    /// "at 2:30".
    At { time: Time },
    /// "@clip 3f2a91c0".
    Clip { clip: ClipId },
    /// "@marker 9c1d2e3f" or "@marker intro hook".
    Marker { marker: MarkerId },
    /// "@track V1" or "@track 3f2a91c0".
    Track { track: TrackId },
    /// "@media 3f2a91c0" (resolved with the project only).
    Media { media: MediaId },
    /// "@bin 3f2a91c0" (resolved with the project only).
    Bin { bin: BinId },
    /// "@effect 3f2a91c0/9c1d2e3f": a video or audio effect on a clip.
    Effect { clip: ClipId, effect: EffectId },
    /// "@transition 3f2a91c0/in" or ".../out": the clip's transition at
    /// that edge, which may not be set.
    Transition {
        clip: ClipId,
        edge: crate::command::Edge,
    },
    /// "@fx cross_dissolve" (a built-in key) or "@fx 3f2a91c0" (a project
    /// asset, resolved with the project only).
    Fx { fx: crate::fx::FxRef },
    /// "@audiofx compressor": an audio effect kind by its name.
    AudioFx { kind: crate::audiofx::AudioFxKind },
    /// An `@` mention that names nothing in the timeline, or more than one
    /// thing.
    Unresolved { text: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResolvedRef {
    /// Byte range in the message.
    pub span: Range<usize>,
    pub reference: Reference,
}

/// Find every time and `@` reference in `text`, in order. Single times need
/// "at" in front, so ratios like "16:9" are left alone; clock times need a
/// colon ("SS" alone is not a time). Ids match by prefix of their hex form.
/// `@range <start>-<end>` and `@time <secs>` (timeline seconds) read as
/// [`Reference::Range`] and [`Reference::At`]. Media, bins and project FX
/// need the project: see [`resolve_project_references`].
pub fn resolve_references(tl: &Timeline, text: &str) -> Vec<ResolvedRef> {
    resolve(tl, None, text)
}

/// [`resolve_references`] with the whole project, so `@media`, `@bin` and
/// project `@fx` tokens resolve too.
pub fn resolve_project_references(project: &Project, text: &str) -> Vec<ResolvedRef> {
    resolve(&project.timeline, Some(project), text)
}

fn resolve(tl: &Timeline, project: Option<&Project>, text: &str) -> Vec<ResolvedRef> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        let at_boundary = text[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric() && c != ':' && c != '.');
        let found = if rest.starts_with('@') {
            parse_mention(tl, rest).or_else(|| parse_fx_mention(tl, project, rest))
        } else if at_boundary {
            parse_time_ref(rest)
        } else {
            None
        };
        match found {
            Some((len, reference)) => {
                out.push(ResolvedRef {
                    span: i..i + len,
                    reference,
                });
                i += len;
            }
            None => i += rest.chars().next().map_or(1, char::len_utf8),
        }
    }
    out
}

fn parse_time_ref(s: &str) -> Option<(usize, Reference)> {
    let lead = ["at", "from"]
        .iter()
        .find_map(|w| keyword(s, w))
        .unwrap_or(0);
    let (len, start) = clock(&s[lead..])?;
    let mut end_ix = lead + len;
    let after = &s[end_ix..];
    let gap = after.len() - after.trim_start().len();
    let sep = ["-", "–", "—"]
        .iter()
        .find_map(|d| after.trim_start().starts_with(d).then_some(d.len()))
        .or_else(|| keyword(after.trim_start(), "to"));
    if let Some(sep) = sep {
        let tail = &after[gap + sep..];
        let gap2 = tail.len() - tail.trim_start().len();
        if let Some((len2, end)) = clock(&tail[gap2..])
            && end > start
        {
            end_ix += gap + sep + gap2 + len2;
            return Some((
                end_ix,
                Reference::Range {
                    range: TimeRange::new(start, end),
                },
            ));
        }
    }
    (lead > 0 && s[..lead].trim().eq_ignore_ascii_case("at"))
        .then_some((end_ix, Reference::At { time: start }))
}

/// `M:SS`, `H:MM:SS`, with an optional fraction on the seconds.
fn clock(s: &str) -> Option<(usize, Time)> {
    let len = s
        .find(|c: char| !(c.is_ascii_digit() || c == ':' || c == '.'))
        .unwrap_or(s.len());
    let token = s[..len].trim_end_matches(['.', ':']);
    let parts: Vec<&str> = token.split(':').collect();
    let valid = (2..=3).contains(&parts.len())
        && parts.iter().all(|p| !p.is_empty())
        && parts[..parts.len() - 1]
            .iter()
            .all(|p| p.bytes().all(|b| b.is_ascii_digit()))
        && parts[1..]
            .iter()
            .all(|p| p.split('.').next().is_some_and(|w| w.len() == 2));
    valid
        .then(|| Time::parse_clock(token))
        .flatten()
        .map(|t| (token.len(), t))
}

/// `word` at the start of `s`, case-insensitive and followed by whitespace;
/// returns the length including that whitespace.
fn keyword(s: &str, word: &str) -> Option<usize> {
    let head = s.get(..word.len())?;
    let rest = &s[word.len()..];
    let gap = rest.len() - rest.trim_start().len();
    (head.eq_ignore_ascii_case(word) && gap > 0).then_some(word.len() + gap)
}

fn parse_mention(tl: &Timeline, s: &str) -> Option<(usize, Reference)> {
    let body = &s[1..];
    if let Some(k) = keyword(body, "clip") {
        let (len, hex) = hex_token(&body[k..])?;
        let hits: Vec<ClipId> = tl
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .map(|c| c.id)
            .filter(|id| id.0.simple().to_string().starts_with(&hex))
            .collect();
        let reference = match hits[..] {
            [clip] => Reference::Clip { clip },
            _ => unresolved(&s[..1 + k + len]),
        };
        return Some((1 + k + len, reference));
    }
    if let Some(k) = keyword(body, "marker") {
        let rest = &body[k..];
        if let Some((len, hex)) = hex_token(rest) {
            let hits: Vec<&Marker> = tl
                .markers
                .iter()
                .filter(|m| m.id.0.simple().to_string().starts_with(&hex))
                .collect();
            if let [m] = hits[..] {
                return Some((1 + k + len, Reference::Marker { marker: m.id }));
            }
        }
        let quoted = rest
            .strip_prefix('"')
            .and_then(|q| q.find('"').map(|e| (&q[..e], e + 2)));
        let best = match quoted {
            Some((note, len)) => tl
                .markers
                .iter()
                .find(|m| m.note.trim().eq_ignore_ascii_case(note.trim()))
                .map(|m| (len, m.id)),
            None => tl
                .markers
                .iter()
                .filter(|m| !m.note.trim().is_empty())
                .filter_map(|m| {
                    let len = prefix_ci(rest, m.note.trim())?;
                    rest[len..]
                        .chars()
                        .next()
                        .is_none_or(|c| !c.is_alphanumeric())
                        .then_some((len, m.id))
                })
                .max_by_key(|(len, _)| *len),
        };
        return Some(match best {
            Some((len, marker)) => (1 + k + len, Reference::Marker { marker }),
            None => {
                let len = quoted.map_or_else(|| word_len(rest), |(_, l)| l);
                (1 + k + len, unresolved(&s[..1 + k + len]))
            }
        });
    }
    if let Some(k) = keyword(body, "track") {
        let len = word_len(&body[k..]);
        if len == 0 {
            return None;
        }
        if let Some((hex_len, hex)) = hex_token(&body[k..])
            && let [track] = tl
                .tracks
                .iter()
                .filter(|t| t.id.0.simple().to_string().starts_with(&hex))
                .map(|t| t.id)
                .collect::<Vec<_>>()[..]
        {
            return Some((1 + k + hex_len, Reference::Track { track }));
        }
        let name = &body[k..k + len];
        let hits: Vec<TrackId> = tl
            .tracks
            .iter()
            .filter(|t| t.name.eq_ignore_ascii_case(name))
            .map(|t| t.id)
            .collect();
        let reference = match hits[..] {
            [track] => Reference::Track { track },
            _ => unresolved(&s[..1 + k + len]),
        };
        return Some((1 + k + len, reference));
    }
    None
}

/// The mention kinds added with FX: media, bins, effects, transitions, FX
/// assets, audio effects, `@range` and `@time`.
fn parse_fx_mention(
    tl: &Timeline,
    project: Option<&Project>,
    s: &str,
) -> Option<(usize, Reference)> {
    use crate::audiofx::AudioFxKind;
    use crate::command::Edge;
    let body = &s[1..];
    let done = |k: usize, len: usize, r: Option<Reference>| {
        let end = 1 + k + len;
        Some((end, r.unwrap_or_else(|| unresolved(&s[..end]))))
    };
    let clips = || tl.tracks.iter().flat_map(|t| &t.clips);
    if let Some(k) = keyword(body, "media") {
        let (len, hex) = hex_token(&body[k..])?;
        let hit = project.and_then(|p| unique(p.media.iter().map(|m| (m.id, m.id.0)), &hex));
        return done(k, len, hit.map(|media| Reference::Media { media }));
    }
    if let Some(k) = keyword(body, "bin") {
        let (len, hex) = hex_token(&body[k..])?;
        let hit = project.and_then(|p| unique(p.bins.iter().map(|b| (b.id, b.id.0)), &hex));
        return done(k, len, hit.map(|bin| Reference::Bin { bin }));
    }
    if let Some(k) = keyword(body, "effect") {
        let rest = &body[k..];
        let (clip_len, clip_hex) = hex_token(rest)?;
        let (fx_len, fx_hex) = hex_token(rest[clip_len..].strip_prefix('/')?)?;
        let len = clip_len + 1 + fx_len;
        let hit = clips()
            .filter(|c| c.id.0.simple().to_string().starts_with(&clip_hex))
            .collect::<Vec<_>>();
        let found = match hit[..] {
            [clip] => unique(
                clip.effects
                    .iter()
                    .map(|e| (e.id, e.id.0))
                    .chain(clip.audio.effects.iter().map(|e| (e.id, e.id.0))),
                &fx_hex,
            )
            .map(|effect| Reference::Effect {
                clip: clip.id,
                effect,
            }),
            _ => None,
        };
        return done(k, len, found);
    }
    if let Some(k) = keyword(body, "transition") {
        let rest = &body[k..];
        let (clip_len, clip_hex) = hex_token(rest)?;
        let side = rest[clip_len..].strip_prefix('/')?;
        let side_len = word_len(side);
        let edge = match side[..side_len].to_ascii_lowercase().as_str() {
            "in" => Edge::Start,
            "out" => Edge::End,
            _ => return None,
        };
        let clip = unique(clips().map(|c| (c.id, c.id.0)), &clip_hex);
        return done(
            k,
            clip_len + 1 + side_len,
            clip.map(|clip| Reference::Transition { clip, edge }),
        );
    }
    if let Some(k) = keyword(body, "fx") {
        let len = word_len(&body[k..]);
        if len == 0 {
            return None;
        }
        let name = body[k..k + len].to_ascii_lowercase();
        let builtin = crate::fx::builtins()
            .iter()
            .find(|a| matches!(&a.id, crate::fx::FxRef::Builtin(key) if *key == name))
            .map(|a| a.id.clone());
        let fx = builtin.or_else(|| {
            let p = project?;
            let ids = p.fx.iter().filter_map(|a| match &a.id {
                crate::fx::FxRef::Project(id) => Some((*id, id.0)),
                crate::fx::FxRef::Builtin(_) => None,
            });
            unique(ids, &name).map(crate::fx::FxRef::Project)
        });
        return done(k, len, fx.map(|fx| Reference::Fx { fx }));
    }
    if let Some(k) = keyword(body, "audiofx") {
        let len = word_len(&body[k..]);
        if len == 0 {
            return None;
        }
        let kind = AudioFxKind::from_name(&body[k..k + len].to_ascii_lowercase());
        return done(k, len, kind.map(|kind| Reference::AudioFx { kind }));
    }
    if let Some(k) = keyword(body, "range") {
        let rest = &body[k..];
        let (a_len, start) = seconds(rest)?;
        let (b_len, end) = seconds(rest[a_len..].strip_prefix('-')?)?;
        return (end > start).then_some((
            1 + k + a_len + 1 + b_len,
            Reference::Range {
                range: TimeRange::new(start, end),
            },
        ));
    }
    if let Some(k) = keyword(body, "time") {
        let (len, time) = seconds(&body[k..])?;
        return Some((1 + k + len, Reference::At { time }));
    }
    None
}

/// The one id whose hex form starts with `hex`.
fn unique<T: Copy>(ids: impl Iterator<Item = (T, uuid::Uuid)>, hex: &str) -> Option<T> {
    let hits: Vec<T> = ids
        .filter(|(_, u)| u.simple().to_string().starts_with(hex))
        .map(|(id, _)| id)
        .collect();
    match hits[..] {
        [one] => Some(one),
        _ => None,
    }
}

/// Plain seconds such as `12` or `12.345`, ending at a word boundary.
fn seconds(s: &str) -> Option<(usize, Time)> {
    let len = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let token = s[..len].trim_end_matches('.');
    let boundary = s[token.len()..]
        .chars()
        .next()
        .is_none_or(|c| !c.is_alphanumeric());
    let secs: f64 = token.parse().ok().filter(|v: &f64| v.is_finite())?;
    boundary.then(|| (token.len(), Time::from_secs_f64(secs)))
}

fn unresolved(text: &str) -> Reference {
    Reference::Unresolved {
        text: text.to_owned(),
    }
}

/// Lowercased hex id prefix (4–32 digits) ending at a word boundary.
fn hex_token(s: &str) -> Option<(usize, String)> {
    let len = s.find(|c: char| !c.is_ascii_hexdigit()).unwrap_or(s.len());
    let boundary = s[len..].chars().next().is_none_or(|c| !c.is_alphanumeric());
    ((4..=32).contains(&len) && boundary).then(|| (len, s[..len].to_ascii_lowercase()))
}

/// Length of the leading name-like word (letters, digits, `-`, `_`).
fn word_len(s: &str) -> usize {
    s.find(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_'))
        .unwrap_or(s.len())
}

/// Byte length of `prefix` at the start of `s`, compared case-insensitively.
fn prefix_ci(s: &str, prefix: &str) -> Option<usize> {
    let mut chars = s.char_indices();
    let mut end = 0;
    for p in prefix.chars() {
        let (i, c) = chars.next()?;
        if !c.to_lowercase().eq(p.to_lowercase()) {
            return None;
        }
        end = i + c.len_utf8();
    }
    Some(end)
}

// -------------------------------------------------------------- style rules

/// The approved rules in force for `project` (PRD S3), oldest first.
///
/// Rules linked by `supersedes` (through approved rules only) are one
/// conflict group; each group contributes its approved rule in the narrowest
/// scope that applies (this project > next video > global), the newest on a
/// tie. Rules scoped to other projects never apply, so a project rule that
/// supersedes a global one only wins inside its own project.
pub fn effective_rules(rules: &[StyleRule], project: ProjectId) -> Vec<&StyleRule> {
    let index: HashMap<RuleId, usize> = rules.iter().enumerate().map(|(i, r)| (r.id, i)).collect();
    let mut parent: Vec<usize> = (0..rules.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for (i, r) in rules.iter().enumerate() {
        if r.status != RuleStatus::Approved {
            continue;
        }
        if let Some(&j) = r.supersedes.and_then(|s| index.get(&s)) {
            let (a, b) = (root(&mut parent, i), root(&mut parent, j));
            parent[a] = b;
        }
    }
    let rank = |r: &StyleRule| match &r.scope {
        RuleScope::Global => Some(0),
        RuleScope::NextVideo => Some(1),
        RuleScope::Project { project: p } if *p == project => Some(2),
        RuleScope::Project { .. } => None,
    };
    let mut best: HashMap<usize, (u8, i64, usize)> = HashMap::new();
    for (i, r) in rules.iter().enumerate() {
        if r.status != RuleStatus::Approved {
            continue;
        }
        let Some(rank) = rank(r) else { continue };
        let key = (rank, r.created_at, i);
        let group = root(&mut parent, i);
        let slot = best.entry(group).or_insert(key);
        if key > *slot {
            *slot = key;
        }
    }
    let mut out: Vec<&StyleRule> = best.values().map(|&(_, _, i)| &rules[i]).collect();
    out.sort_by_key(|r| (r.created_at, r.id));
    out
}
