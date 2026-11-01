//! Carrying out an approved agent request: memory goes to the store,
//! programs and sounds to the user's library, renders and exports to `koo-deliver`,
//! music and sound effects to the provider set for them. Outputs always get
//! a free name, so nothing here overwrites a file.

use crate::persistence;
use anyhow::{Context as _, Result, anyhow, bail};
use koo_agent::tools::ApprovalRequest;
use koo_agent::tools::generate::{self, Placement};
use koo_core::agent::AgentSettings;
use koo_core::memory::{
    CaptionStyle, FactKind, FactStatus, Recipe, RecipeParam, RuleScope, RuleStatus, SourceFact,
    StyleRule,
};
use koo_core::model::{Fingerprint, MediaItem, Project};
use koo_core::services::Services;
use koo_core::{JobId, RecipeId, RuleId, Time, TimeRange};
use koo_deliver::{RenderQueue, RenderRequest, RenderSettings};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// What approving started.
pub enum Started {
    /// Done at once; one line for the card.
    Done(String),
    /// Queued in the render queue, writing this file.
    Render(JobId, PathBuf),
    /// Runs off the UI thread and yields the line for the card.
    Background(Box<dyn FnOnce() -> Result<String> + Send>),
    /// Runs off the UI thread and yields a file a provider made, for the
    /// pool, with where to place it.
    Generate(Box<dyn FnOnce() -> Result<Generated> + Send>),
}

/// What an approved generation made.
pub struct Generated {
    pub item: MediaItem,
    pub placement: Option<Placement>,
    pub summary: String,
}

pub struct Inputs<'a> {
    pub project: &'a Project,
    pub caption_styles: &'a [CaptionStyle],
    pub queue: &'a RenderQueue,
    pub services: &'a Services,
    /// May gain a style profile when a rule needs one.
    pub settings: &'a mut AgentSettings,
}

pub fn start(request: &ApprovalRequest, inputs: Inputs) -> Result<Started> {
    let args = &request.args;
    match request.tool.as_str() {
        "propose_rule" => save_rule(args, inputs.project, inputs.settings),
        "add_source_fact" => save_fact(args),
        "save_recipe" => save_recipe(args),
        "save_fx_to_library" => save_fx(args, inputs.project),
        "save_sound_to_library" => save_sound(args, inputs.project),
        "queue_render" => {
            let mut settings = preset(str_arg(args, "preset")?);
            settings.range = args.get("range").and_then(range);
            let output = free_output(inputs.project, settings.extension());
            let id = inputs.queue.enqueue(RenderRequest {
                project: inputs.project.clone(),
                settings,
                output: output.clone(),
                caption_styles: inputs.caption_styles.to_vec(),
                services: inputs.services.clone(),
            })?;
            Ok(Started::Render(id, output))
        }
        "generate_audio" => {
            let (request, placement) = generate::from_json(args)
                .ok_or_else(|| anyhow!("the request could not be read"))?;
            let generation = inputs
                .services
                .generation
                .clone()
                .ok_or_else(|| anyhow!("generation is not available"))?;
            let media = inputs.services.media.clone();
            Ok(Started::Generate(Box::new(move || {
                let made = generation
                    .generate(&request, &koo_core::paths::generated())
                    .map_err(|e| anyhow!(generate::error_text(&e)))?;
                Ok(Generated {
                    item: koo_core::cloud::media_item(&made, media.as_deref()),
                    placement,
                    summary: request.summary(),
                })
            })))
        }
        "export_otio" => {
            let project = inputs.project.clone();
            let path = free_output(&project, "otio");
            Ok(Started::Background(Box::new(move || {
                koo_deliver::export_otio(&project, &path)?;
                Ok(file_name(&path))
            })))
        }
        "export_frame" => {
            let t = args
                .get("t")
                .and_then(Value::as_f64)
                .map(Time::from_secs_f64)
                .ok_or_else(|| anyhow!("missing time"))?;
            let project = inputs.project.clone();
            let styles = inputs.caption_styles.to_vec();
            let services = inputs.services.clone();
            let path = free_output(&project, "png");
            Ok(Started::Background(Box::new(move || {
                koo_deliver::export_frame(&project, t, &styles, &services, &path)?;
                Ok(file_name(&path))
            })))
        }
        other => bail!("`{other}` cannot be approved"),
    }
}

fn save_rule(args: &Value, project: &Project, settings: &mut AgentSettings) -> Result<Started> {
    let profile = match settings.style_profile {
        Some(id) => id,
        None => {
            let profile = store(|s| s.create_profile("Default"))?;
            settings.style_profile = Some(profile.id);
            profile.id
        }
    };
    let scope = match args.get("scope").and_then(Value::as_str) {
        Some("project") => RuleScope::Project {
            project: project.id,
        },
        Some("next_video") => RuleScope::NextVideo,
        _ => RuleScope::Global,
    };
    let supersedes = match args.get("supersedes").and_then(Value::as_str) {
        Some(short) => store(|s| {
            s.rules(profile, &Default::default())
                .map(|rules| find_rule(&rules, short))
        })?,
        None => None,
    };
    let rule = StyleRule {
        id: RuleId::new(),
        text: str_arg(args, "text")?.to_owned(),
        scope,
        status: RuleStatus::Proposed,
        supersedes,
        created_at: koo_store::now(),
        evidence: strings(args.get("evidence")),
    };
    // Approving through the store also retires the rule it supersedes.
    store(|s| {
        s.save_rule(profile, &rule)?;
        s.set_rule_status(rule.id, RuleStatus::Approved)
    })?;
    Ok(Started::Done("Saved".into()))
}

fn find_rule(rules: &[StyleRule], short: &str) -> Option<RuleId> {
    let short = short.trim().to_ascii_lowercase();
    (short.len() >= 4)
        .then(|| {
            rules
                .iter()
                .find(|r| r.id.0.simple().to_string().starts_with(&short))
                .map(|r| r.id)
        })
        .flatten()
}

fn save_fact(args: &Value) -> Result<Started> {
    let kind: FactKind = serde_json::from_value(args.get("kind").cloned().unwrap_or_default())
        .context("unknown fact kind")?;
    let fact = SourceFact {
        id: Default::default(),
        fingerprint: Fingerprint(str_arg(args, "fingerprint")?.to_owned()),
        kind,
        text: str_arg(args, "text")?.to_owned(),
        range: args.get("source_range").and_then(range),
        status: FactStatus::Confirmed,
    };
    store(|s| s.save_fact(&fact))?;
    Ok(Started::Done("Saved".into()))
}

fn save_recipe(args: &Value) -> Result<Started> {
    let name = str_arg(args, "name")?.to_owned();
    let params = args
        .get("params")
        .and_then(Value::as_array)
        .map(|params| params.iter().filter_map(param).collect())
        .unwrap_or_default();
    let recipe = Recipe {
        id: RecipeId::new(),
        description: args
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        params,
        prompt: str_arg(args, "prompt")?.to_owned(),
        builtin: false,
        name: name.clone(),
    };
    store(|s| s.save_recipe(&recipe))?;
    Ok(Started::Done("Saved".into()))
}

/// Save a project program to the user's library. The editor links the
/// project's copy to it afterwards (see `EditorState::approve`).
fn save_fx(args: &Value, project: &Project) -> Result<Started> {
    let text = str_arg(args, "fx")?;
    let asset = koo_core::fx::find(project, text)
        .and_then(|r| koo_core::fx::asset(project, &r))
        .ok_or_else(|| anyhow!("program {text} is no longer in the project"))?;
    let lib = koo_core::library::Library::load();
    koo_core::library::save_program(&lib, asset).map_err(|e| anyhow!(e))?;
    Ok(Started::Done("Saved to the library".into()))
}

/// Save audio from the pool to the user's library.
fn save_sound(args: &Value, project: &Project) -> Result<Started> {
    let text = str_arg(args, "media")?;
    let item = uuid::Uuid::parse_str(text)
        .ok()
        .and_then(|id| project.media(koo_core::MediaId(id)))
        .ok_or_else(|| anyhow!("media {text} is no longer in the project"))?;
    let lib = koo_core::library::Library::load();
    koo_core::library::save_sound(&lib, item).map_err(|e| anyhow!(e))?;
    Ok(Started::Done("Saved to the library".into()))
}

/// A recipe parameter, as a bare name or `{name, description, default}`.
fn param(v: &Value) -> Option<RecipeParam> {
    if let Some(name) = v.as_str() {
        return Some(RecipeParam {
            name: name.to_owned(),
            description: String::new(),
            default: None,
        });
    }
    let text = |key| v.get(key).and_then(Value::as_str).map(str::to_owned);
    Some(RecipeParam {
        name: text("name")?,
        description: text("description").unwrap_or_default(),
        default: text("default"),
    })
}

/// The deliver preset closest to the agent's name for it
/// (`youtube_1080p`, `4k`, `prores`, `wav`…).
fn preset(name: &str) -> RenderSettings {
    let key: String = name
        .to_lowercase()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    RenderSettings::presets()
        .into_iter()
        .find(|p| {
            let preset: String = p
                .name
                .to_lowercase()
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .collect();
            preset == key
        })
        .unwrap_or_else(|| {
            if key.contains("prores") {
                RenderSettings::prores_422_hq()
            } else if key.contains("wav") || key.contains("audio") {
                RenderSettings::wav()
            } else if key.contains("4k") || key.contains("2160") {
                RenderSettings::h264_4k30()
            } else if key.contains("30") {
                RenderSettings::h264_1080p30()
            } else {
                RenderSettings::h264_1080p60()
            }
        })
}

/// `<project name>.<ext>` in the user's video folder, numbered if taken.
fn free_output(project: &Project, ext: &str) -> PathBuf {
    let stem: String = project
        .name
        .trim()
        .chars()
        .map(|c| if r#"/\:*?"<>|"#.contains(c) { '_' } else { c })
        .collect();
    let stem = if stem.is_empty() { "Untitled" } else { &stem };
    koo_deliver::free_name(&output_dir().join(format!("{stem}.{ext}")))
}

fn output_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let videos = home.join(if cfg!(target_os = "macos") {
        "Movies"
    } else {
        "Videos"
    });
    if videos.is_dir() { videos } else { home }
}

pub fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn store<T>(f: impl FnOnce(&koo_store::Store) -> koo_store::Result<T>) -> Result<T> {
    persistence::with_store(f).ok_or_else(|| {
        anyhow!(persistence::take_notice().unwrap_or_else(|| "The store is unavailable".into()))
    })
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("missing `{key}`"))
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn range(v: &Value) -> Option<TimeRange> {
    let at = |key| v.get(key).and_then(Value::as_f64).map(Time::from_secs_f64);
    Some(TimeRange::new(at("start")?, at("end")?))
}
