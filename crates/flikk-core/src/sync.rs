//! Syncing recordings by their sound: a second camera or a separate audio
//! recorder lined up with the main clip (see [`measure`]), and the edit
//! that puts them in place ([`plan`]).

pub mod fft;
pub mod measure;
pub mod plan;
pub mod varispeed;

pub use measure::{MIN_CONFIDENCE, Measured, Source};

use crate::ids::MediaId;
use crate::model::Project;
use crate::services::MediaService;

/// The recording of `media`'s `stream`, whole.
pub fn source(project: &Project, media: MediaId, stream: u32) -> Option<Source> {
    Source::whole(project.media(media)?, stream)
}

/// Each follower against `reference`, by their sound. A follower that
/// doesn't match the reference (it doesn't overlap it, or hears it badly)
/// is tried through another follower that did. `Err` says why one could
/// not be synced; a result under [`MIN_CONFIDENCE`] is an `Err` too.
pub fn measure_group(
    media: &dyn MediaService,
    reference: &Source,
    followers: &[Source],
) -> Result<Vec<Result<Measured, String>>, String> {
    let name = |s: &Source| {
        s.item
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let reference_envelope = measure::envelope(media, reference).map_err(|e| e.0)?;
    let envelopes: Vec<Result<measure::Envelope, String>> = followers
        .iter()
        .map(|f| measure::envelope(media, f).map_err(|e| e.0))
        .collect();
    let mut out: Vec<Result<Measured, String>> = followers
        .iter()
        .zip(&envelopes)
        .map(|(f, e)| {
            let e = e.as_ref().map_err(Clone::clone)?;
            let m = measure::measure(media, reference, &reference_envelope, f, e)
                .map_err(|e| format!("{}: {}", name(f), e.0))?;
            if m.matched() {
                Ok(m)
            } else {
                Err(format!("{}: no match", name(f)))
            }
        })
        .collect();
    // Through a follower that matched: b = oB + a·(1 + dB), a = oA + r·(1 + dA).
    for i in 0..followers.len() {
        if out[i].is_ok() {
            continue;
        }
        let Ok(env_b) = &envelopes[i] else {
            continue;
        };
        for j in 0..followers.len() {
            let (Ok(a), Ok(env_a)) = (&out[j], &envelopes[j]) else {
                continue;
            };
            if i == j {
                continue;
            }
            let Ok(b) = measure::measure(media, &followers[j], env_a, &followers[i], env_b) else {
                continue;
            };
            if !b.matched() {
                continue;
            }
            let joined = Measured {
                offset: b.offset + a.offset * (1.0 + b.drift),
                drift: (1.0 + a.drift) * (1.0 + b.drift) - 1.0,
                confidence: a.confidence.min(b.confidence),
                overlap: a.overlap,
                windows: b.windows,
                agreeing: b.agreeing,
            };
            out[i] = Ok(joined);
            break;
        }
    }
    Ok(out)
}

/// The pool items worth syncing among `media` (with sound, no repeats),
/// the reference first: the first video, else the first item.
pub fn pool_order(project: &Project, media: &[MediaId]) -> Vec<MediaId> {
    use crate::model::MediaKind;
    let mut out: Vec<MediaId> = Vec::new();
    for id in media {
        let Some(item) = project.media(*id) else {
            continue;
        };
        let sound = item.info.as_ref().is_some_and(|i| !i.audio.is_empty());
        if item.kind != MediaKind::Image && sound && !out.contains(id) {
            out.push(*id);
        }
    }
    let video = out.iter().position(|m| {
        project
            .media(*m)
            .is_some_and(|i| i.kind == MediaKind::Video)
    });
    if let Some(i) = video.filter(|i| *i > 0) {
        let v = out.remove(i);
        out.insert(0, v);
    }
    out
}

/// What syncing a group came to: the command to apply (none when nothing
/// matched) and, per follower, its result.
pub struct Synced {
    pub command: Option<crate::command::Command>,
    pub results: Vec<Result<Measured, String>>,
}

/// [`Command::SyncClips`](crate::command::Command::SyncClips) for the
/// timeline `recordings` (reference first) and what was measured of the
/// followers; with `mute_camera`, the cameras' sound is disabled when a
/// recorder matched.
pub fn clips_command(
    project: &Project,
    recordings: &[plan::Recording],
    results: Vec<Result<Measured, String>>,
    mute_camera: bool,
) -> Synced {
    let Some(reference) = recordings.first() else {
        return Synced {
            command: None,
            results,
        };
    };
    let matched: Vec<&plan::Recording> = recordings[1..]
        .iter()
        .zip(&results)
        .filter(|(_, r)| r.is_ok())
        .map(|(rec, _)| rec)
        .collect();
    let followers: Vec<plan::SyncedClip> = recordings[1..]
        .iter()
        .zip(&results)
        .filter_map(|(rec, r)| {
            let m = r.as_ref().ok()?;
            Some(plan::SyncedClip {
                clip: rec.clip,
                offset: m.offset,
                drift: m.drift,
            })
        })
        .collect();
    let command = (!followers.is_empty()).then(|| {
        let mut group = vec![reference.clone()];
        group.extend(matched.into_iter().cloned());
        let mute = if mute_camera {
            plan::camera_sound(project, &group)
        } else {
            Vec::new()
        };
        crate::command::Command::SyncClips {
            reference: reference.clip,
            followers,
            mute,
        }
    });
    Synced { command, results }
}

/// [`Command::PlaceSynced`](crate::command::Command::PlaceSynced) for pool
/// items in [`pool_order`] placed from `at`; with `mute_camera`, the videos'
/// sound is placed disabled when a sound-only item matched.
pub fn place_command(
    project: &Project,
    media: &[MediaId],
    at: crate::time::Time,
    results: Vec<Result<Measured, String>>,
    mute_camera: bool,
) -> Synced {
    use crate::model::MediaKind;
    let Some(&reference) = media.first() else {
        return Synced {
            command: None,
            results,
        };
    };
    let followers: Vec<plan::SyncedMedia> = media[1..]
        .iter()
        .zip(&results)
        .filter_map(|(id, r)| {
            let m = r.as_ref().ok()?;
            Some(plan::SyncedMedia {
                media: *id,
                offset: m.offset,
                drift: m.drift,
            })
        })
        .collect();
    let kind = |id: MediaId| project.media(id).map(|m| m.kind);
    let placed: Vec<MediaId> = std::iter::once(reference)
        .chain(followers.iter().map(|f| f.media))
        .collect();
    let recorder = placed.iter().any(|m| kind(*m) == Some(MediaKind::Audio));
    let mute = if mute_camera && recorder {
        placed
            .iter()
            .copied()
            .filter(|m| kind(*m) == Some(MediaKind::Video))
            .collect()
    } else {
        Vec::new()
    };
    let command = (!followers.is_empty()).then_some(crate::command::Command::PlaceSynced {
        at,
        reference,
        followers,
        mute,
    });
    Synced { command, results }
}
