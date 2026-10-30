//! `generate_audio`: music or a sound effect made by the provider the user
//! set for it (the Generate category). Each one spends the user's credits,
//! so it waits for their approval unless they allowed it. The file goes to
//! the pool's Generated bin; with `at` (or `range`) it is also proposed on
//! an audio track.

use super::{Done, Job, JobDone, Step, ToolCtx};
use crate::args::{Args, ToolError, ToolResultOf, fail, secs, short, track_id};
use koo_core::bus::{Author, CommandBus};
use koo_core::cloud::{CloudError, GenerateRequest, MusicSection};
use koo_core::command::{Command, InsertMode};
use koo_core::model::{MediaItem, Project, TrackKind};
use koo_core::{MediaId, Time, TimeRange, TrackId};
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
        other => return fail(format!("unknown kind `{other}`: music or sound_effect")),
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
    let job: Job =
        Box::new(
            move || match generation.generate(&request, &koo_core::paths::generated()) {
                Ok(made) => JobDone::Generated {
                    item: Box::new(koo_core::cloud::media_item(&made, media.as_deref())),
                    placement,
                    summary: request.summary(),
                },
                Err(e) => JobDone::Done(Done::error(error_text(&e))),
            },
        );
    Ok(Step::Job(job))
}

/// Why the user's settings stop it.
pub fn unavailable(request: &GenerateRequest) -> String {
    format!(
        "No provider with an API key is set for {} in Settings → Integrations. Tell the user; don't retry.",
        request.capability().label().to_lowercase()
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
    let length = item.info.as_ref().map_or(0.0, |i| i.duration.as_secs_f64());
    let media = match add_to_pool(bus, item) {
        Ok(media) => media,
        Err(e) => return Done::error(format!("Made it, but could not add it to the pool: {e}")),
    };
    let mut text = format!(
        "Made it: media {} ({length:.1} s) in the Generated bin.",
        short(media)
    );
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
    Done::text(format!("generated {summary}"), text)
}

/// Add `item` to the pool's Generated bin as the user's edit (it is paid
/// for: rejecting a placement keeps it), and to the drafts waiting for
/// review so a proposal can use it.
pub fn add_to_pool(bus: &mut CommandBus, item: MediaItem) -> Result<MediaId, String> {
    let id = item.id;
    let commands = koo_core::cloud::pool_commands(bus.project(), item);
    bus.execute_shared(commands, "Generate audio")
        .map(|_| id)
        .map_err(|e| e.to_string())
}

/// Propose `media` on the timeline as `placement` says, as `author`: on its
/// track, else on an audio track with nothing over that time, else on a new
/// one. What happened, for the model.
pub fn propose_placement(
    bus: &mut CommandBus,
    author: &Author,
    media: MediaId,
    placement: &Placement,
    reason: &str,
) -> Result<String, String> {
    let project = bus.view_for(author);
    let length = project
        .media(media)
        .and_then(|m| m.info.as_ref())
        .map(|i| i.duration)
        .filter(|d| *d > Time::ZERO)
        .ok_or("the generated file could not be read")?;
    let span = TimeRange::new(placement.at, placement.at + length);
    let free = placement.track.or_else(|| {
        project
            .timeline
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Audio && !t.locked)
            .find(|t| t.clips.iter().all(|c| !c.range().overlaps(&span)))
            .map(|t| t.id)
    });
    let before: Vec<TrackId> = project.timeline.tracks.iter().map(|t| t.id).collect();
    let execute = |bus: &mut CommandBus, command: Command| -> Result<(), String> {
        bus.execute(command, author.clone(), reason)
            .map(|_| ())
            .map_err(|e| e.to_string())
    };
    let track = match free {
        Some(track) => track,
        None => {
            execute(
                bus,
                Command::AddTrack {
                    kind: TrackKind::Audio,
                    index: before.len(),
                    name: "Generated".into(),
                },
            )?;
            bus.view_for(author)
                .timeline
                .tracks
                .iter()
                .find(|t| !before.contains(&t.id))
                .map(|t| t.id)
                .ok_or("could not add an audio track")?
        }
    };
    execute(
        bus,
        Command::InsertClip {
            media,
            source: TimeRange::new(Time::ZERO, length),
            track,
            at: placement.at,
            mode: InsertMode::Overwrite,
        },
    )?;
    let text = format!(
        "Proposed it over {:.3}–{:.3} s on track {}.",
        span.start.as_secs_f64(),
        span.end.as_secs_f64(),
        short(track)
    );
    Ok(text)
}
