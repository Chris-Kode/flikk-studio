//! What a cached frame shows, as a hash. The timeline is cut wherever the
//! set of clips that can show changes (clip edges, transition edges); each
//! piece that needs the compositor is a [`Span`] whose hash covers
//! everything drawn there, and a frame's [`Key`] is that hash with the
//! frame's index.
//!
//! The hash is taken over serialized state, not over chosen fields: the
//! clips that show (and their touching neighbours, which transitions
//! reach into) as they are saved, their tracks, their media, the timeline
//! apart from its tracks, the project's programs, the caption styles, the
//! render size and which engines drew it. A clip property added later is
//! part of the key without anyone listing it, so an edit only ever changes
//! keys; it never leaves a stale frame under a key that still matches.
//! Only fields known to leave the picture alone are left out (clip audio,
//! names and colours; markers and ducking; bins and tags).

use koo_core::compose;
use koo_core::fx;
use koo_core::memory::CaptionStyle;
use koo_core::model::{Clip, ClipContent, Project, Track, TrackKind};
use koo_core::services::Services;
use koo_core::{ClipId, MaskId, MediaId, Rate, Time, TimeRange};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::ops::Range;

/// A cached frame's name.
pub type Key = [u8; 16];

/// Bumped when the file layout or what the hash covers changes.
const FORMAT: u32 = 1;
/// Short side of cached frames, at most the sequence's.
const SHORT_SIDE: u32 = 720;

/// What every frame shares that lives outside the project: caption styles,
/// the engines that draw and the subject model. Cheap to compare.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Globals([u8; 32]);

impl Globals {
    pub fn new(services: &Services, styles: &[CaptionStyle]) -> Self {
        let mut h = blake3::Hasher::new();
        h.update(&FORMAT.to_le_bytes());
        h.update(env!("CARGO_PKG_VERSION").as_bytes());
        h.update(&[
            services.fx.is_some() as u8,
            services.html.is_some() as u8,
            services.lettering.is_some() as u8,
            services.subjects.is_some() as u8,
            services.media.is_some() as u8,
            // Without the flow model, retimed frames are blended instead.
            services
                .interpolation
                .as_ref()
                .is_some_and(|i| i.available().is_ok()) as u8,
            // Without it, stabilized clips draw as they are.
            services.stabilizer.is_some() as u8,
        ]);
        h.update(koo_models::Task::Segmentation.model().id.as_bytes());
        hash_json(&mut h, styles);
        Self(*h.finalize().as_bytes())
    }
}

/// A stretch of frames that needs the compositor, drawn the same way
/// throughout but for time.
pub struct Span {
    pub frames: Range<i64>,
    hash: [u8; 32],
    /// Media the frames decode: they wait until it is online.
    pub media: Vec<MediaId>,
    /// Subject and person masks shown: their frames wait until every
    /// matte is made.
    pub subjects: Vec<(ClipId, MaskId)>,
    /// Stabilized clips shown: their frames wait until each camera path is
    /// measured (until then they draw unstabilized).
    pub stabilized: Vec<ClipId>,
}

/// The keyed spans of one project at one size.
pub struct Plan {
    pub globals: Globals,
    pub rate: Rate,
    /// Cached frames' size.
    pub size: (u32, u32),
    /// In order, never overlapping.
    pub spans: Vec<Span>,
}

impl Plan {
    pub fn new(project: &Project, globals: Globals) -> Self {
        let tl = &project.timeline;
        let rate = tl.settings.rate;
        let size = size(project);
        let mut base = blake3::Hasher::new();
        base.update(&globals.0);
        base.update(&size.0.to_le_bytes());
        base.update(&size.1.to_le_bytes());
        let mut timeline = to_value(tl);
        strip(&mut timeline, &["tracks", "markers", "ducking"]);
        hash_value(&mut base, &timeline);
        hash_json(&mut base, &project.fx);

        // Every track that shows: pictures, titles and captions.
        let tracks: Vec<(usize, &Track)> = tl
            .tracks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.kind != TrackKind::Audio)
            .collect();
        let track_hashes: Vec<[u8; 32]> = tracks
            .iter()
            .map(|(i, t)| {
                let mut h = blake3::Hasher::new();
                h.update(&(*i as u64).to_le_bytes());
                let mut v = to_value(t);
                strip(&mut v, &["clips", "name", "locked"]);
                hash_value(&mut h, &v);
                *h.finalize().as_bytes()
            })
            .collect();

        let end = frame_at_or_after(tl.duration(), rate);
        let mut cuts = vec![0, end];
        for (_, track) in &tracks {
            for clip in &track.clips {
                cuts.push(frame_at_or_after(clip.start, rate));
                cuts.push(frame_at_or_after(clip.end(), rate));
            }
            for placed in fx::transitions(track) {
                cuts.push(frame_at_or_after(placed.range.start, rate));
                cuts.push(frame_at_or_after(placed.range.end, rate));
            }
        }
        cuts.retain(|f| (0..=end).contains(f));
        cuts.sort_unstable();
        cuts.dedup();

        let mut clip_hashes: HashMap<ClipId, [u8; 32]> = HashMap::new();
        let mut spans = Vec::new();
        for pair in cuts.windows(2) {
            let (f0, f1) = (pair[0], pair[1]);
            // From the first frame's time up to just past the last's.
            let range = TimeRange::new(
                Time::from_frame(f0, rate),
                Time::from_frame(f1 - 1, rate) + Time(1),
            );
            if !compose::needs_compositor(project, range) {
                continue;
            }
            let mut h = base.clone();
            let mut media: Vec<MediaId> = Vec::new();
            let mut subjects = Vec::new();
            let mut stabilized = Vec::new();
            for ((_, track), track_hash) in tracks.iter().zip(&track_hashes) {
                // Clips are sorted and never overlap, so those shown are
                // one run.
                let mut shown = track
                    .clips
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c.range().overlaps(&range))
                    .map(|(i, _)| i);
                let Some(first) = shown.next() else {
                    continue;
                };
                let last = shown.next_back().unwrap_or(first);
                h.update(track_hash);
                // With the clips they touch, which transitions draw from.
                let lo = first.saturating_sub(1);
                let hi = (last + 1).min(track.clips.len() - 1);
                for (i, clip) in track.clips.iter().enumerate().take(hi + 1).skip(lo) {
                    let inside = (first..=last).contains(&i);
                    let touching = (i < first && clip.end() == track.clips[first].start)
                        || (i > last && clip.start == track.clips[last].end());
                    if !inside && !touching {
                        continue;
                    }
                    let ch = clip_hashes
                        .entry(clip.id)
                        .or_insert_with(|| clip_hash(project, clip));
                    h.update(ch);
                    if let ClipContent::Media { media: id, .. } = &clip.content
                        && !media.contains(id)
                    {
                        media.push(*id);
                    }
                    if inside {
                        subjects.extend(
                            clip.masks
                                .iter()
                                .filter(|m| m.enabled && (m.picks().is_some() || m.is_person()))
                                .map(|m| (clip.id, m.id)),
                        );
                    }
                    if (inside || touching) && koo_core::stabilize::applies(clip) {
                        stabilized.push(clip.id);
                    }
                }
            }
            spans.push(Span {
                frames: f0..f1,
                hash: *h.finalize().as_bytes(),
                media,
                subjects,
                stabilized,
            });
        }
        Self {
            globals,
            rate,
            size,
            spans,
        }
    }

    /// The span holding `frame`, if it needs the compositor.
    pub fn span(&self, frame: i64) -> Option<&Span> {
        let i = self.spans.partition_point(|s| s.frames.end <= frame);
        self.spans.get(i).filter(|s| s.frames.contains(&frame))
    }

    /// Where `frame` is cached, if it is drawn by the compositor.
    pub fn key(&self, frame: i64) -> Option<Key> {
        self.span(frame).map(|s| s.key(frame))
    }
}

impl Span {
    pub fn key(&self, frame: i64) -> Key {
        let mut h = blake3::Hasher::new();
        h.update(&self.hash);
        h.update(&frame.to_le_bytes());
        let mut key = [0; 16];
        key.copy_from_slice(&h.finalize().as_bytes()[..16]);
        key
    }
}

/// Cached frames' size: the sequence's shape at [`SHORT_SIDE`], or smaller
/// when the sequence is.
pub fn size(project: &Project) -> (u32, u32) {
    let s = &project.timeline.settings;
    let short = s.width.min(s.height).max(1);
    let width = s.width as u64 * SHORT_SIDE.min(short) as u64 / short as u64;
    compose::size_for_width(project, width as u32)
}

/// The first frame whose time is at or after `t`.
fn frame_at_or_after(t: Time, rate: Rate) -> i64 {
    let d = rate.frame_duration().0.max(1);
    (t.0 + d - 1).div_euclid(d)
}

/// A clip as saved, without what only the sound or the timeline's look
/// uses, and its media as the pool has it.
fn clip_hash(project: &Project, clip: &Clip) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    let mut v = to_value(clip);
    strip(&mut v, &["audio", "name", "color"]);
    hash_value(&mut h, &v);
    if let ClipContent::Media { media, .. } = &clip.content
        && let Some(item) = project.media(*media)
    {
        let mut v = to_value(item);
        strip(&mut v, &["bin", "tags", "generated"]);
        hash_value(&mut h, &v);
    }
    *h.finalize().as_bytes()
}

/// Through a [`Value`], whose maps keep their keys sorted, so the bytes
/// are the same for the same state whatever maps the model uses.
fn to_value<T: Serialize + ?Sized>(x: &T) -> Value {
    serde_json::to_value(x).unwrap_or(Value::Null)
}

fn strip(v: &mut Value, fields: &[&str]) {
    if let Value::Object(map) = v {
        for f in fields {
            map.remove(*f);
        }
    }
}

fn hash_json<T: Serialize + ?Sized>(h: &mut blake3::Hasher, x: &T) {
    hash_value(h, &to_value(x));
}

fn hash_value(h: &mut blake3::Hasher, v: &Value) {
    let bytes = serde_json::to_vec(v).unwrap_or_default();
    h.update(&(bytes.len() as u64).to_le_bytes());
    h.update(&bytes);
}
