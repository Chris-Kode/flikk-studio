//! Fit a song to a length (Premiere's Remix): the song ends when the video
//! does, on its real ending, with seamless edits instead of a hard cut or a
//! fade.
//!
//! The song's bars come from the beat tracker's downbeats (analysis), each
//! moved onto the attack nearby. [`features`] describes each bar (chroma,
//! MFCC and loudness from PCM decoded here), [`planner`] scores bar-to-bar
//! jumps on a self-similarity matrix and finds the cheapest path of the
//! right length that keeps the intro and the ending, jumping on phrase
//! boundaries and looping sections to grow. The path becomes
//! [`Command::FitMusic`]: the clip replaced by segments back to back, an
//! equal-power crossfade ending on each jump, and a fade only when the
//! ending's ring had to be trimmed. Undo, change sets and the agent all
//! see ordinary clips.

pub mod features;
pub mod planner;

pub use planner::{Fit, FitOptions, Jump, JumpKind, Song};

use crate::analysis::Analysis;
use crate::command::{Command, Plan};
use crate::error::{CoreError, Result};
use crate::fx::{FxInstance, FxRef, Transition, TransitionAlign};
use crate::ids::{ClipId, TrackId};
use crate::model::{Clip, ClipContent, MediaItem, Project, TrackKind};
use crate::services::{MediaService, Services};
use crate::time::{Time, TimeRange};

/// Where the fitted song should end.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    /// The end of everything on the other tracks.
    SequenceEnd,
    /// From the range's start to its end (an in/out range).
    Range(TimeRange),
    /// This long from the clip's start.
    Duration(Time),
    /// Ending at this timeline time.
    End(Time),
}

/// A music clip and what it should become.
#[derive(Clone, Debug)]
pub struct Request {
    pub clip: Clip,
    pub track: TrackId,
    pub item: MediaItem,
    /// Timeline start of the fitted song.
    pub at: Time,
    pub length: Time,
    pub options: FitOptions,
}

impl Request {
    /// Check the clip and resolve `target` on `project`; cheap, for the UI
    /// thread.
    pub fn new(
        project: &Project,
        clip: ClipId,
        target: Target,
        options: FitOptions,
    ) -> std::result::Result<Self, String> {
        let (track, c) = project
            .timeline
            .clip(clip)
            .ok_or_else(|| format!("No clip {clip}."))?;
        if track.kind != TrackKind::Audio {
            return Err("Only clips on audio tracks can be fitted.".into());
        }
        if track.locked {
            return Err(format!("Track {} is locked.", track.name));
        }
        let item = c
            .media()
            .and_then(|m| project.media(m))
            .ok_or("Only media clips can be fitted.")?
            .clone();
        if (c.speed - 1.0).abs() > 1e-6 {
            return Err("Set the clip's speed back to 100% first; fitting keeps the tempo.".into());
        }
        let (at, end) = match target {
            Target::SequenceEnd => {
                let end = project
                    .timeline
                    .tracks
                    .iter()
                    .filter(|t| t.id != track.id)
                    .flat_map(|t| t.clips.iter().filter(|c| c.enabled))
                    .map(Clip::end)
                    .max()
                    .ok_or("Nothing on the other tracks to fit the song to.")?;
                (c.start, end)
            }
            Target::Range(r) => (r.start, r.end),
            Target::Duration(d) => (c.start, c.start + d),
            Target::End(t) => (c.start, t),
        };
        if end <= at {
            return Err("The song would have to end before it starts.".into());
        }
        Ok(Self {
            clip: c.clone(),
            track: track.id,
            item,
            at,
            length: end - at,
            options,
        })
    }

    fn stream(&self) -> u32 {
        match &self.clip.content {
            ClipContent::Media {
                audio_stream: Some(s),
                ..
            } => *s,
            _ => self.item.first_audio_stream().unwrap_or(0),
        }
    }

    /// Decode, describe the bars and plan (slow: run it off the UI thread).
    pub fn fit(&self, services: &Services) -> std::result::Result<(Command, Fit), String> {
        let media = services
            .media
            .as_ref()
            .ok_or("Decoding audio is not available in this build.")?;
        let analysis = self
            .item
            .fingerprint
            .as_ref()
            .and_then(|fp| services.analysis.as_ref()?.analysis(fp))
            .ok_or("Analyze the song first: fitting needs its beats.")?;
        let source_in = match &self.clip.content {
            ClipContent::Media { source_in, .. } => (*source_in).max(Time::ZERO),
            _ => Time::ZERO,
        };
        let song = song(
            media.as_ref(),
            &self.item,
            self.stream(),
            &analysis,
            source_in,
        )?;
        let fit = planner::plan(&song, self.length, &self.options)?;
        let command = Command::FitMusic {
            clip: self.clip.id,
            at: self.at,
            segments: fit.segments.clone(),
            crossfade: self.options.crossfade,
            fade_out: fit.fade_out,
        };
        Ok((command, fit))
    }
}

/// The song from `from` to the end of the media, cut into bars.
pub fn song(
    media: &dyn MediaService,
    item: &MediaItem,
    stream: u32,
    analysis: &Analysis,
    from: Time,
) -> std::result::Result<Song, String> {
    let end = item
        .info
        .as_ref()
        .map(|i| i.duration)
        .filter(|d| *d > from)
        .ok_or("The song's length is not known.")?;
    let mut downbeats: Vec<Time> = analysis
        .music
        .iter()
        .flat_map(|m| m.downbeats.iter().copied())
        .filter(|t| *t >= from && *t < end)
        .collect();
    downbeats.sort();
    downbeats.dedup();
    let mut beats: Vec<Time> = analysis
        .music
        .iter()
        .flat_map(|m| m.beats.iter().copied())
        .collect();
    beats.sort();
    if downbeats.len() < 9 {
        return Err(if analysis.music.is_empty() {
            "Analysis heard no music in this clip.".into()
        } else {
            "Analysis found too few bars in this clip to fit it.".into()
        });
    }
    let beat = typical_gap(&beats).unwrap_or(0.5);
    let bar = typical_gap(&downbeats).unwrap_or(beat * 4.0);
    // Fill bars the tracker missed (quiet passages, gaps between stretches
    // of music) where the gap holds a whole number of bars.
    let mut filled = vec![downbeats[0]];
    for w in downbeats.windows(2) {
        let gap = (w[1] - w[0]).as_secs_f64();
        let k = (gap / bar).round();
        if k >= 2.0 && (gap / bar - k).abs() < 0.2 {
            for m in 1..k as i64 {
                filled.push(w[0] + Time::from_secs_f64(gap * m as f64 / k));
            }
        }
        filled.push(w[1]);
    }

    let pcm = decode(media, item, stream, from, end)?;
    let secs = |t: Time| (t - from).as_secs_f64();
    let refined: Vec<Time> = filled
        .iter()
        .map(|t| from + Time::from_secs_f64(features::attack(&pcm, secs(*t))))
        .collect();
    let bars: Vec<TimeRange> = refined
        .windows(2)
        .map(|w| TimeRange::new(w[0], w[1]))
        .collect();
    let regular: Vec<bool> = bars
        .iter()
        .map(|b| (b.duration().as_secs_f64() / bar - 1.0).abs() < 0.12)
        .collect();
    let spans: Vec<(f64, f64)> = bars.iter().map(|b| (secs(b.start), secs(b.end))).collect();
    let mut features = features::bars(&pcm, &spans);
    let mut bars = bars;
    let mut regular: Vec<bool> = regular;
    // The tracker hears bars in the ring of a final chord: bars well below
    // the song's usual level at its end belong to the ending.
    let mut levels: Vec<f32> = features.iter().map(|f| f.loudness_db).collect();
    levels.sort_by(f32::total_cmp);
    let usual = levels[levels.len() / 2];
    while bars.len() > 8 && features.last().is_some_and(|f| f.loudness_db < usual - 8.0) {
        bars.pop();
        regular.pop();
        features.pop();
    }
    Ok(Song {
        source: TimeRange::new(from, end),
        bars,
        regular,
        features,
        beat: Time::from_secs_f64(beat),
    })
}

/// Mono samples at [`features::RATE`] of `[from, end)`.
fn decode(
    media: &dyn MediaService,
    item: &MediaItem,
    stream: u32,
    from: Time,
    end: Time,
) -> std::result::Result<Vec<f32>, String> {
    let mut pcm = media
        .audio(item, stream, from, features::RATE)
        .map_err(|e| e.to_string())?;
    let want = ((end - from).as_secs_f64() * features::RATE as f64) as usize;
    let mut out = Vec::with_capacity(want);
    let mut buf = vec![0f32; 8192];
    while out.len() < want {
        let n = pcm.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        out.extend(
            buf[..n]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|[l, r]| 0.5 * (l + r)),
        );
    }
    out.truncate(want);
    if out.len() < features::RATE as usize {
        return Err("The song's audio could not be read.".into());
    }
    Ok(out)
}

/// Mean of the gaps within 10 % of the median, in seconds.
fn typical_gap(times: &[Time]) -> Option<f64> {
    let mut gaps: Vec<f64> = times
        .windows(2)
        .map(|w| (w[1] - w[0]).as_secs_f64())
        .filter(|g| *g > 0.0)
        .collect();
    if gaps.len() < 3 {
        return None;
    }
    gaps.sort_by(f64::total_cmp);
    let median = gaps[gaps.len() / 2];
    let near: Vec<f64> = gaps
        .into_iter()
        .filter(|g| (g - median).abs() <= median * 0.1)
        .collect();
    Some(near.iter().sum::<f64>() / near.len() as f64)
}

/// One line per edit, for the agent and the status line.
pub fn describe(fit: &Fit) -> Vec<String> {
    let mut out = Vec::new();
    for j in &fit.jumps {
        let what = match j.kind {
            JumpKind::Skip => "skips",
            JumpKind::Loop => "repeats",
        };
        out.push(format!(
            "at {:.2} s: {what} source {:.2}–{:.2} s ({:.1} s), match {:.0}%",
            j.at.as_secs_f64(),
            j.span.start.as_secs_f64(),
            j.span.end.as_secs_f64(),
            j.span.duration().as_secs_f64(),
            j.quality * 100.0
        ));
    }
    if fit.trimmed > Time::ZERO {
        out.push(format!(
            "ending: {:.2} s of its ring trimmed under a {:.2} s fade",
            fit.trimmed.as_secs_f64(),
            fit.fade_out.as_secs_f64()
        ));
    }
    let err = fit.error.as_secs_f64();
    if err.abs() >= 0.005 {
        out.push(format!(
            "ends {:.2} s {} the target",
            err.abs(),
            if err < 0.0 { "before" } else { "after" }
        ));
    }
    out
}

/// Plan [`Command::FitMusic`]: the clip replaced by its segments back to
/// back from `at`, an equal-power crossfade ending on each join.
pub(crate) fn plan_command(
    project: &Project,
    clip: ClipId,
    at: Time,
    segments: &[TimeRange],
    crossfade: Time,
    fade_out: Time,
) -> Result<Plan> {
    let tl = &project.timeline;
    let (track, original) = tl.clip(clip).ok_or(CoreError::ClipNotFound(clip))?;
    if track.locked {
        return Err(CoreError::Invalid(format!(
            "track {} is locked",
            track.name
        )));
    }
    if track.kind != TrackKind::Audio {
        return Err(CoreError::Invalid(
            "only clips on audio tracks can be fitted".into(),
        ));
    }
    let ClipContent::Media {
        media,
        audio_stream,
        ..
    } = original.content.clone()
    else {
        return Err(CoreError::Invalid("only media clips can be fitted".into()));
    };
    let length = crate::edit::media_duration(project, media);
    if segments.is_empty() || at < Time::ZERO {
        return Err(CoreError::Invalid(
            "no segments, or a negative start".into(),
        ));
    }
    if segments.iter().any(|s| {
        s.is_empty() || s.start < Time::ZERO || length.is_some_and(|l| s.end > l + Time(1))
    }) {
        return Err(CoreError::Invalid(
            "a segment is empty or outside the media".into(),
        ));
    }
    let crossfade = crossfade.max(Time::ZERO);
    let mut cursor = at;
    let mut placed = Vec::with_capacity(segments.len());
    for (k, seg) in segments.iter().enumerate() {
        let mut c = original.clone();
        c.id = ClipId::new();
        c.start = cursor;
        c.duration = seg.duration();
        c.speed = 1.0;
        c.content = ClipContent::Media {
            media,
            source_in: seg.start,
            audio_stream,
        };
        // Keyframes were placed on the old clip's time.
        c.keyframes.clear();
        let first = k == 0;
        let last = k + 1 == segments.len();
        c.transition_in = if first {
            original.transition_in.clone()
        } else {
            (crossfade > Time::ZERO).then(|| Transition {
                fx: FxInstance::new(FxRef::builtin("cross_dissolve")),
                duration: crossfade.min(c.duration),
                align: TransitionAlign::EndOnCut,
            })
        };
        c.transition_out = if last {
            original.transition_out.clone()
        } else {
            None
        };
        c.audio.fade_in = if first {
            original.audio.fade_in.min(c.duration)
        } else {
            Time::ZERO
        };
        c.audio.fade_out = if last {
            fade_out.max(Time::ZERO).min(c.duration)
        } else {
            Time::ZERO
        };
        cursor = cursor + c.duration;
        placed.push(c);
    }
    let span = TimeRange::new(at, cursor);
    if let Some(other) = track
        .clips
        .iter()
        .find(|c| c.id != clip && c.range().overlaps(&span))
    {
        return Err(CoreError::Invalid(format!(
            "the fitted song ({span}) would run over {} on {}",
            other.display_name(project),
            track.name
        )));
    }
    let mut next: Vec<Clip> = track
        .clips
        .iter()
        .filter(|c| c.id != clip)
        .cloned()
        .collect();
    next.extend(placed);
    Ok(Plan {
        ops: crate::edit::diff_track(track, next),
        warnings: Vec::new(),
    })
}
