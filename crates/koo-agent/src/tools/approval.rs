//! Tools that need the user: approval requests (project and memory,
//! deliver, generating unless allowed) and clarifying questions.

use super::{ApprovalRequest, Done, ToolCtx};
use crate::args::{Args, ToolError, media_id, range_json};
use koo_core::agent::AgentEvent;
use koo_core::command::ToolCategory;
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

/// Validate the arguments, then queue the request for the UI.
pub fn request(
    ctx: &mut ToolCtx,
    tool: &str,
    category: ToolCategory,
    args: &Args,
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
        "save_recipe" => {
            let name = args.str("name")?.trim().trim_start_matches('/');
            if name.is_empty() || name.contains(char::is_whitespace) {
                return Err(ToolError("`name` is one word, like rough-cut".into()));
            }
            (
                format!("Recipe /{name}"),
                json!({
                    "name": name,
                    "description": args.opt_str("description")?.unwrap_or_default(),
                    "prompt": args.str("prompt")?,
                    "params": args.0.get("params").cloned().unwrap_or(json!([])),
                }),
            )
        }
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
            (
                format!("Render {preset} ({range})"),
                json!({ "preset": preset, "range": range_json(range) }),
            )
        }
        "save_fx_to_library" => {
            let asset = super::fx::fx_asset(project, args.str("fx")?, None)?;
            if !matches!(asset.id, koo_core::fx::FxRef::Project(_)) {
                return Err(ToolError(
                    "built-ins are in every project already; save a project program".into(),
                ));
            }
            (
                format!("Save “{}” to the library", asset.name),
                json!({ "fx": asset.id.to_string() }),
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
            (
                super::generate::summary(&request, placement.as_ref()),
                super::generate::request_json(&request, placement.as_ref()),
            )
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
    let mut done = Done::text(
        format!("awaiting approval: {summary}"),
        "Queued for the user's approval. It is not done yet; don't wait for it, and mention it in your reply.",
    );
    done.approval = Some(ApprovalRequest {
        tool: tool.to_owned(),
        category,
        args: normalized,
        summary,
    });
    Ok(done)
}
