//! `generate_audio`: music, a sound effect or a voiceover made by the
//! provider the user set for it (the Generate category). Each one spends the
//! user's credits, so each call waits for their approval.
//! The file goes to the pool's bin for what it is; with `at` (or `range`) it
//! is also proposed on an audio track. Voices are found and made in
//! `voices`.

use crate::args::{Args, ToolError, ToolResultOf, fail, secs, short, track_id};
use flikk_core::bus::{Author, CommandBus};
use flikk_core::cloud::{self, Capability, CloudError, GenerateRequest, MusicSection, Reading};
use flikk_core::command::{Command, InsertMode};
use flikk_core::model::{MediaKind, Project, TrackKind};
use flikk_core::{MediaId, Time, TimeRange, TrackId};
use serde_json::{Value, json};

/// Where a generated file goes on the timeline.
#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    pub at: Time,
    /// `None`: an audio track free over its length, else a new one.
    pub track: Option<TrackId>,
}

impl Placement {
    pub(crate) fn to_json(&self) -> Value {
        json!({ "at": secs(self.at), "track": self.track.map(|t| t.0.to_string()) })
    }

    pub(crate) fn from_json(value: &Value) -> Option<Self> {
        let at = Time::from_secs_f64(value.get("at")?.as_f64()?);
        let track = value
            .get("track")
            .and_then(Value::as_str)
            .and_then(|t| uuid::Uuid::parse_str(t).ok())
            .map(TrackId);
        Some(Self { at, track })
    }
}

/// Where a generated file goes once it is in the pool.
#[derive(Clone, Debug, PartialEq)]
pub enum Destination {
    /// Only the pool.
    Pool,
    /// Proposed on the timeline as well.
    Timeline(Placement),
    /// Proposed in place of this video in its clips: it is a version of it
    /// (an upscale).
    Replace(MediaId),
}

impl From<Option<Placement>> for Destination {
    fn from(placement: Option<Placement>) -> Self {
        placement.map_or(Self::Pool, Self::Timeline)
    }
}

/// The request and its placement, from the tool's arguments.
pub(crate) fn parse(
    project: &Project,
    args: &Args,
) -> ToolResultOf<(GenerateRequest, Option<Placement>)> {
    let prompt = args.str("prompt")?.trim().to_owned();
    if prompt.is_empty() {
        return fail("`prompt` describes what to make");
    }
    let range = args.opt_range("range")?;
    let length = match (args.opt_f64("duration")?, range) {
        (Some(secs), _) if secs > 0.0 => Some(Time::from_secs_f64(secs)),
        (Some(_), _) => return fail("`duration` is in seconds, more than 0"),
        (None, Some(r)) => Some(r.duration()),
        (None, None) => None,
    };
    let request = match args.str("kind")? {
        "music" => GenerateRequest::Music {
            prompt,
            length,
            instrumental: args.opt_bool("instrumental")?.unwrap_or(true),
            sections: sections(args)?,
        },
        // The lengths and looping the model takes are checked against the
        // model the user picked, before approval.
        "sound_effect" => GenerateRequest::SoundEffect {
            prompt,
            length,
            looping: args.opt_bool("loop")?.unwrap_or(false),
        },
        "voiceover" => {
            cloud::check_script(&prompt).map_err(|e| ToolError(error_text(&e)))?;
            let voice = args.str("voice")?.trim().to_owned();
            let unit = |key: &str, default: f32| -> ToolResultOf<f32> {
                match args.opt_f64(key)? {
                    Some(v) if (0.0..=1.0).contains(&v) => Ok(v as f32),
                    Some(_) => fail(format!("`{key}` runs from 0 to 1")),
                    None => Ok(default),
                }
            };
            GenerateRequest::Voiceover {
                reading: Reading {
                    voice_name: args.opt_str("voice_name")?.unwrap_or(&voice).to_owned(),
                    stability: unit("stability", Reading::STABILITY)?,
                    similarity: unit("similarity", Reading::SIMILARITY)?,
                    voice,
                },
                text: prompt,
                context: None,
            }
        }
        other => {
            return fail(format!(
                "unknown kind `{other}`: music, sound_effect or voiceover"
            ));
        }
    };
    let at = args.opt_time("at")?.or(range.map(|r| r.start));
    let track = args
        .opt_str("track")?
        .map(|t| track_id(project, t))
        .transpose()?;
    if let Some(track) = track
        && project
            .timeline
            .track(track)
            .is_some_and(|t| t.kind != TrackKind::Audio)
    {
        return fail("`track` must be an audio track");
    }
    let placement = at.map(|at| Placement { at, track });
    Ok((request, placement))
}

fn sections(args: &Args) -> ToolResultOf<Vec<MusicSection>> {
    let Some(Value::Array(items)) = args.value("sections") else {
        return Ok(Vec::new());
    };
    items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let part = Args::of(item, args.1)
                .map_err(|_| ToolError(format!("`sections[{i}]` must be an object")))?;
            let seconds = part.opt_f64("duration")?.unwrap_or(0.0);
            if seconds <= 0.0 {
                return fail(format!(
                    "`sections[{i}].duration` is in seconds, more than 0"
                ));
            }
            Ok(MusicSection {
                name: part
                    .opt_str("name")?
                    .map_or_else(|| format!("Part {}", i + 1), str::to_owned),
                length: Time::from_secs_f64(seconds),
                styles: part.strings("styles")?,
                avoid: part.strings("avoid")?,
                lyrics: part.strings("lyrics")?,
            })
        })
        .collect()
}

/// One line for the approval card and the activity log.
pub(crate) fn summary(request: &GenerateRequest, placement: Option<&Placement>) -> String {
    match placement {
        Some(p) => format!("{} → {:.1}s", request.summary(), p.at.as_secs_f64()),
        None => request.summary(),
    }
}

/// The request with its placement, as an approval carries it.
pub(crate) fn request_json(request: &GenerateRequest, placement: Option<&Placement>) -> Value {
    json!({
        "request": request,
        "place": placement.map(Placement::to_json),
    })
}

/// What [`request_json`] made.
pub fn from_json(value: &Value) -> Option<(GenerateRequest, Option<Placement>)> {
    let request = serde_json::from_value(value.get("request")?.clone()).ok()?;
    let placement = value.get("place").and_then(Placement::from_json);
    Some((request, placement))
}

/// Why the user's settings stop it.
pub(crate) fn unavailable(request: &GenerateRequest) -> String {
    unavailable_for(request.capability())
}

pub(crate) fn unavailable_for(capability: Capability) -> String {
    format!(
        "No provider with an API key is set for {} in Settings → Integrations. Tell the user; don't retry.",
        capability.label().to_lowercase()
    )
}

/// A provider's refusal as the model should read it.
pub fn error_text(e: &CloudError) -> String {
    match e {
        CloudError::BadPrompt {
            message,
            suggestion: Some(s),
        } => format!("Refused: {message}. The provider suggests instead: {s}"),
        CloudError::BadPrompt { message, .. } => {
            format!("Refused: {message}. Describe the sound without naming artists or songs.")
        }
        e if e.needs_settings() => format!("{e}. The user must fix it in Settings → Integrations."),
        e => e.to_string(),
    }
}

/// Propose `media` where `destination` says, as `author`; what happened,
/// for the model. Nothing for [`Destination::Pool`].
pub fn propose_destination(
    bus: &mut CommandBus,
    author: &Author,
    media: MediaId,
    destination: &Destination,
    reason: &str,
) -> Result<String, String> {
    match destination {
        Destination::Pool => Ok(String::new()),
        Destination::Timeline(placement) => {
            propose_placement(bus, author, media, placement, reason)
        }
        Destination::Replace(from) => propose_replacement(bus, author, *from, media, reason),
    }
}

/// Propose that the clips of `from` on picture tracks show `to` instead,
/// as `author`. What happened, for the model.
fn propose_replacement(
    bus: &mut CommandBus,
    author: &Author,
    from: MediaId,
    to: MediaId,
    reason: &str,
) -> Result<String, String> {
    let clips = (bus.view_for(author).timeline)
        .picture_clips_of(from)
        .count();
    if clips == 0 {
        return Ok(format!(
            "No clip on an unlocked picture track shows media {}, so none was replaced.",
            short(from)
        ));
    }
    bus.execute(Command::SwapMedia { from, to }, author.clone(), reason)
        .map_err(|e| e.to_string())?;
    let what = if clips == 1 { "clip" } else { "clips" };
    Ok(format!(
        "Proposed it in place of media {} in {clips} {what}; their sound stays on it.",
        short(from)
    ))
}

/// Propose `media` on the timeline as `placement` says, as `author`: on its
/// track, else on a track of its kind with nothing over that time, else on
/// a new one. A video with sound puts its sound on an audio track the same
/// way. What happened, for the model.
pub fn propose_placement(
    bus: &mut CommandBus,
    author: &Author,
    media: MediaId,
    placement: &Placement,
    reason: &str,
) -> Result<String, String> {
    let project = bus.view_for(author);
    let item = project.media(media).ok_or("the file is not in the pool")?;
    let info = item.info.as_ref();
    let length = info
        .map(|i| i.duration)
        .filter(|d| *d > Time::ZERO)
        .ok_or("the length of the file could not be read")?;
    let video = item.kind == MediaKind::Video;
    let sound = video && info.is_some_and(|i| !i.audio.is_empty());
    let span = TimeRange::new(placement.at, placement.at + length);
    let insert = |bus: &mut CommandBus, track| {
        bus.execute(
            Command::InsertClip {
                media,
                source: TimeRange::new(Time::ZERO, length),
                track,
                at: placement.at,
                mode: InsertMode::Overwrite,
            },
            author.clone(),
            reason,
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    };
    let kind = if video {
        TrackKind::Video
    } else {
        TrackKind::Audio
    };
    let track = match placement.track {
        Some(track) => track,
        None => free_track(bus, author, kind, span, reason)?,
    };
    insert(bus, track)?;
    let mut text = format!(
        "Proposed it over {:.3}–{:.3} s on track {}",
        span.start.as_secs_f64(),
        span.end.as_secs_f64(),
        short(track)
    );
    if sound {
        let audio = free_track(bus, author, TrackKind::Audio, span, reason)?;
        insert(bus, audio)?;
        text.push_str(&format!(", its sound on track {}", short(audio)));
    }
    text.push('.');
    Ok(text)
}

/// An unlocked track of `kind` with nothing over `span`, else a new one
/// (proposed as `author`): a video track on top, an audio track at the
/// bottom.
fn free_track(
    bus: &mut CommandBus,
    author: &Author,
    kind: TrackKind,
    span: TimeRange,
    reason: &str,
) -> Result<TrackId, String> {
    let project = bus.view_for(author);
    let free = project
        .timeline
        .tracks
        .iter()
        .filter(|t| t.kind == kind && !t.locked)
        .find(|t| t.clips.iter().all(|c| !c.range().overlaps(&span)))
        .map(|t| t.id);
    if let Some(track) = free {
        return Ok(track);
    }
    let before: Vec<TrackId> = project.timeline.tracks.iter().map(|t| t.id).collect();
    let index = match kind {
        TrackKind::Audio => before.len(),
        _ => 0,
    };
    bus.execute(
        Command::AddTrack {
            kind,
            index,
            name: "Generated".into(),
        },
        author.clone(),
        reason,
    )
    .map_err(|e| e.to_string())?;
    bus.view_for(author)
        .timeline
        .tracks
        .iter()
        .find(|t| !before.contains(&t.id))
        .map(|t| t.id)
        .ok_or_else(|| format!("could not add a {} track", crate::args::kind_name(kind)))
}
