//! Tools that need the user: approval requests (project and memory,
//! deliver, generating unless allowed) and clarifying questions.

use super::{ApprovalRequest, Done, ToolCtx};
use crate::args::{Args, ToolError, media_id, range_json};
use flikk_core::agent::AgentEvent;
use flikk_core::command::ToolCategory;
use flikk_core::model::MediaKind;
use serde_json::{Value, json};

pub fn ask_user(args: &Args) -> Result<Done, ToolError> {
    let question = args.str("question")?.trim().to_owned();
    let choices: Vec<String> = args
        .strings("choices")?
        .into_iter()
        .map(|c| c.trim().to_owned())
        .filter(|c| !c.is_empty())
        .take(6)
        .collect();
    let mut done = Done::text(
        format!("asked: {question}"),
        "The question is shown to the user with the choices. End your turn now; their answer arrives as the next message.",
    );
    done.events.push(AgentEvent::Question {
        text: question,
        choices,
    });
    Ok(done)
}

/// Validate the arguments, then queue the request for the UI: to ask the
/// user, or to carry out at once when they `allowed` its category.
pub fn request(
    ctx: &mut ToolCtx,
    tool: &str,
    category: ToolCategory,
    args: &Args,
    allowed: bool,
) -> Result<Done, ToolError> {
    let project = ctx.project();
    let (summary, normalized): (String, Value) = match tool {
        "propose_rule" => {
            let text = args.str("text")?.trim();
            let scope = args.opt_str("scope")?.unwrap_or("global");
            if !["global", "project", "next_video"].contains(&scope) {
                return Err(ToolError(format!("unknown scope `{scope}`")));
            }
            (
                format!("Style rule: {text}"),
                json!({
                    "text": text,
                    "scope": scope,
                    "evidence": args.strings("evidence")?,
                    "supersedes": args.opt_str("supersedes")?,
                }),
            )
        }
        tool if super::skills::TOOLS.contains(&tool) => super::skills::request(tool, args)?,
        "add_source_fact" => {
            let media = media_id(project, args.str("media")?)?;
            let item = project
                .media(media)
                .ok_or_else(|| ToolError("media not found".into()))?;
            if item.fingerprint.is_none() {
                return Err(ToolError(
                    "This media has no fingerprint yet; facts bind to fingerprints.".into(),
                ));
            }
            let kind = args.str("kind")?;
            if ![
                "speaker_reference",
                "original_audio_track",
                "literal_quote",
                "on_screen",
            ]
            .contains(&kind)
            {
                return Err(ToolError(format!("unknown fact kind `{kind}`")));
            }
            let text = args.str("text")?.trim();
            (
                format!("Source fact: {text}"),
                json!({
                    "media": media.0.to_string(),
                    "fingerprint": item.fingerprint,
                    "kind": kind,
                    "text": text,
                    "source_range": args.opt_range("source_range")?.map(range_json),
                }),
            )
        }
        "queue_render" => {
            let preset = args.str("preset")?;
            let range = args.range_or_all("range", project)?;
            let mut json = json!({ "preset": preset, "range": range_json(range) });
            if let Some(encoder) = args.opt_str("encoder")? {
                json["encoder"] = json!(encoder);
            }
            (format!("Render {preset} ({range})"), json)
        }
        "save_fx_to_library" => {
            let asset = super::fx::fx_asset(project, args.str("fx")?, None)?;
            if !matches!(asset.id, flikk_core::fx::FxRef::Project(_)) {
                return Err(ToolError(
                    "built-ins are in every project already; save a project program".into(),
                ));
            }
            (
                format!("Save “{}” to the library", asset.name),
                json!({ "fx": asset.id.to_string() }),
            )
        }
        "save_sound_to_library" => {
            let id = media_id(project, args.str("media")?)?;
            let item = project.media(id);
            let Some(item) = item.filter(|m| m.kind == MediaKind::Audio) else {
                return Err(ToolError("only audio goes to the library".into()));
            };
            let name = item.path.file_name().unwrap_or_default().to_string_lossy();
            (
                format!("Save “{name}” to the library"),
                json!({ "media": id.0.to_string() }),
            )
        }
        "generate_audio" => {
            let (request, placement) = super::generate::parse(project, args)?;
            let available = ctx
                .services
                .generation
                .as_ref()
                .is_some_and(|g| g.provider_for(request.capability()).is_some());
            if !available {
                return Err(ToolError(super::generate::unavailable(&request)));
            }
            let mut json = super::generate::request_json(&request, placement.as_ref());
            if let flikk_core::cloud::GenerateRequest::Voiceover { reading, .. } = &request {
                json = super::voices::with_sample(json, &reading.voice);
            }
            (super::generate::summary(&request, placement.as_ref()), json)
        }
        "generate_video" => {
            let generation = ctx.services.generation.as_deref();
            let setup = super::video::VideoSetup::of(generation);
            let shot = super::video::parse(project, &setup, args)?;
            let capability = shot.request.capability();
            if !generation.is_some_and(|g| g.provider_for(capability).is_some()) {
                return Err(ToolError(super::generate::unavailable(&shot.request)));
            }
            (
                super::video::summary(&shot),
                super::video::request_json(&shot),
            )
        }
        super::upscale::TOOL => {
            let generation = ctx.services.generation.as_deref();
            let model = generation.and_then(|g| g.upscale_model());
            let (request, replaces) = super::upscale::parse(project, model.as_ref(), args)?;
            if !generation.is_some_and(|g| g.provider_for(request.capability()).is_some()) {
                return Err(ToolError(super::generate::unavailable(&request)));
            }
            (
                super::upscale::summary(&request, replaces),
                super::upscale::request_json(&request, replaces),
            )
        }
        super::image::TOOL => {
            let request = super::image::parse(project, ctx.settings, ctx.services, args)?;
            (
                super::image::summary(&request),
                super::image::request_json(&request),
            )
        }
        "replace_words" => {
            let (fix, reading) = super::respeak::parse(ctx, args)?;
            (
                super::respeak::summary(&fix, &reading),
                super::voices::with_sample(
                    super::respeak::request_json(&fix, &reading),
                    &reading.voice,
                ),
            )
        }
        "add_voice" => {
            super::voices::check_available(ctx)?;
            let adding = super::voices::parse_add(args)?;
            (
                super::voices::add_summary(&adding),
                super::voices::with_sample(super::voices::add_json(&adding), &adding.voice),
            )
        }
        "design_voice" => {
            super::voices::check_available(ctx)?;
            let (design, name) = super::voices::parse_design(args)?;
            (
                super::voices::design_summary(&design, &name),
                super::voices::design_json(&design, &name),
            )
        }
        tool if super::caption_styles::TOOLS.contains(&tool) => {
            super::caption_styles::request(ctx.memory, tool, args)?
        }
        "export_otio" => ("Export OpenTimelineIO".to_owned(), json!({})),
        "export_frame" => {
            let t = args.time("t")?;
            (
                format!("Export frame at {t}"),
                json!({ "t": crate::args::secs(t) }),
            )
        }
        other => return Err(ToolError(format!("`{other}` cannot be requested"))),
    };
    let mut done = if allowed {
        Done::text(summary.clone(), "Saving.")
    } else {
        Done::text(
            format!("awaiting approval: {summary}"),
            "Waiting for the user's approval.",
        )
    };
    done.approval = Some(ApprovalRequest {
        tool: tool.to_owned(),
        category,
        args: normalized,
        summary,
        allowed,
    });
    Ok(done)
}
