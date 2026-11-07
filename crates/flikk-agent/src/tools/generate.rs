//! `generate_audio`: music, a sound effect or a voiceover made by the
//! provider the user set for it (the Generate category). Each one spends the
//! user's credits, so it waits for their approval unless they allowed it.
//! The file goes to the pool's bin for what it is; with `at` (or `range`) it
//! is also proposed on an audio track. Voices are found and made in
//! `voices`.

use super::{Done, Job, JobDone, Step, ToolCtx};
use crate::args::{Args, ToolError, ToolResultOf, fail, secs, short, track_id};
use flikk_core::bus::{Author, CommandBus};
use flikk_core::cloud::{self, Capability, CloudError, GenerateRequest, MusicSection, Reading};
use flikk_core::command::{Command, InsertMode};
use flikk_core::model::{MediaItem, MediaKind, Project, TrackKind};
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
    pub fn to_json(&self) -> Value {
        json!({ "at": secs(self.at), "track": self.track.map(|t| t.0.to_string()) })
    }

    pub fn from_json(value: &Value) -> Option<Self> {
        let at = Time::from_secs_f64(value.get("at")?.as_f64()?);
        let track = value
            .get("track")
            .and_then(Value::as_str)
            .and_then(|t| uuid::Uuid::parse_str(t).ok())
            .map(TrackId);
        Some(Self { at, track })
    }
}

/// The request and its placement, from the tool's arguments.
pub fn parse(project: &Project, args: &Args) -> ToolResultOf<(GenerateRequest, Option<Placement>)> {
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
        "sound_effect" => {
            if length.is_some_and(|l| l > Time::from_secs_f64(30.0)) {
                return fail("A sound effect runs at most 30 s; make it loop, or make music");
            }
            GenerateRequest::SoundEffect {
                prompt,
                length,
                looping: args.opt_bool("loop")?.unwrap_or(false),
            }
        }
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
pub fn summary(request: &GenerateRequest, placement: Option<&Placement>) -> String {
    match placement {
        Some(p) => format!("{} → {:.1}s", request.summary(), p.at.as_secs_f64()),
        None => request.summary(),
    }
}

/// The request with its placement, as an approval carries it.
pub fn request_json(request: &GenerateRequest, placement: Option<&Placement>) -> Value {
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

/// With the user's leave: a job that makes the file and comes back to the
/// UI thread to add it to the pool and place it.
pub fn serve(ctx: &mut ToolCtx, args: &Args) -> ToolResultOf<Step> {
    let (request, placement) = parse(ctx.project(), args)?;
    let Some(generation) = ctx.services.generation.clone() else {
        return fail("Generation is not available in this build.");
    };
    if generation.provider_for(request.capability()).is_none() {
        return fail(unavailable(&request));
    }
    let media = ctx.services.media.clone();
    let task = ctx.stop.task();
    let job: Job = Box::new(move || {
        match generation.generate(&request, &flikk_core::paths::generated(), &task) {
            Ok(made) => JobDone::Generated {
                item: Box::new(flikk_core::cloud::media_item(&made, media.as_deref())),
                placement,
                summary: request.summary(),
            },
            Err(e) => JobDone::Done(Done::error(error_text(&e))),
        }
    });
    Ok(Step::Job(job))
}

/// Why the user's settings stop it.
pub fn unavailable(request: &GenerateRequest) -> String {
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

/// Back on the UI thread with what a provider made: add it to the pool,
/// and place it as `author` (none when the turn was cancelled).
pub fn finish(
    bus: &mut CommandBus,
    author: Option<&Author>,
    item: MediaItem,
    placement: Option<&Placement>,
    summary: &str,
) -> Done {
    let picture = (item.kind == MediaKind::Image).then(|| item.path.clone());
    let length = match (&picture, item.info.as_ref()) {
        (None, Some(info)) => format!(" ({:.1} s)", info.duration.as_secs_f64()),
        _ => String::new(),
    };
    let bin = cloud::generated_bin(&item);
    let media = match add_to_pool(bus, item, author) {
        Ok(media) => media,
        Err(e) => return Done::error(format!("Made it, but could not add it to the pool: {e}")),
    };
    let mut text = format!("Made it: media {}{length} in the {bin} bin.", short(media));
    match (placement, author) {
        (Some(placement), Some(author)) => {
            match propose_placement(bus, author, media, placement, summary) {
                Ok(placed) => {
                    text.push(' ');
                    text.push_str(&placed);
                }
                Err(e) => text.push_str(&format!(" Could not place it: {e}")),
            }
        }
        (Some(_), None) => text.push_str(" The turn was cancelled, so it was not placed."),
        (None, _) => {}
    }
    let mut done = Done::text(format!("generated {summary}"), text);
    done.images.extend(picture);
    done
}

/// Add `item` to the pool's bin for what it is, at once (it is paid for:
/// rejecting a placement keeps it), and to the drafts waiting for review so
/// a proposal can use it. As `author`'s change, so it is no edit of the
/// user's that the agent's proposals would conflict with; the user's when
/// the turn is gone.
pub fn add_to_pool(
    bus: &mut CommandBus,
    item: MediaItem,
    author: Option<&Author>,
) -> Result<MediaId, String> {
    let id = item.id;
    let bin = cloud::generated_bin(&item);
    let label = match item.kind {
        MediaKind::Video => "Generate video",
        MediaKind::Image => "Generate image",
        _ => "Generate audio",
    };
    let commands = cloud::pool_commands(bus.project(), item, bin);
    let author = author.cloned().unwrap_or(Author::User);
    bus.execute_shared_as(commands, label, author)
        .map(|_| id)
        .map_err(|e| e.to_string())
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
