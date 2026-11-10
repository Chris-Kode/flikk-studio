//! Carrying out an approved agent request: memory goes to the store, skills
//! and styles to their files,
//! programs and sounds to the user's library, renders and exports to `flikk-deliver`,
//! music, sound effects, voiceovers, videos and upscales to the provider set for them, as are
//! voices added or designed, which then go to the library; pictures to the
//! user's ChatGPT plan. Outputs always
//! get a free name, so nothing here overwrites a file.

use crate::persistence;
use anyhow::{Context as _, Result, anyhow, bail};
use flikk_agent::tools::ApprovalRequest;
use flikk_agent::tools::generate::{self, Destination};
use flikk_core::agent::AgentSettings;
use flikk_core::memory::{
    CaptionStyle, FactKind, FactStatus, RuleScope, RuleStatus, SourceFact, StyleRule,
};
use flikk_core::model::{Fingerprint, MediaItem, Project};
use flikk_core::services::Services;
use flikk_core::skills::{self, Draft, Kind};
use flikk_core::{JobId, RuleId, Time, TimeRange};
use flikk_deliver::{RenderQueue, RenderRequest, RenderSettings};
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
    /// Runs off the UI thread, adding or designing a voice that lands in
    /// the library; yields what to tell the agent.
    Voice(Box<dyn FnOnce() -> Result<String> + Send>),
    /// Runs off the UI thread, saying a fix's words again; yields them for
    /// the pool, with the fix to cut them in by.
    Respeak(Box<dyn FnOnce() -> Result<Respoken> + Send>),
    /// A change to the style profile's caption styles, for the editor
    /// state to make.
    CaptionStyle(flikk_agent::tools::caption_styles::Op),
}

/// Words an approved fix had said again.
pub struct Respoken {
    pub item: MediaItem,
    pub fix: flikk_core::respeak::Fix,
    pub summary: String,
}

/// What an approved generation made.
pub struct Generated {
    pub item: MediaItem,
    pub destination: Destination,
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
        "save_skill" | "save_style" => save_entry(args),
        "delete_skill" | "delete_style" => delete_entry(args),
        "save_fx_to_library" => save_fx(args, inputs.project),
        "save_sound_to_library" => save_sound(args, inputs.project),
        "queue_render" => {
            let mut settings = preset(str_arg(args, "preset")?);
            settings.range = args.get("range").and_then(range);
            settings.encoder = args
                .get("encoder")
                .and_then(Value::as_str)
                .and_then(flikk_deliver::Encoder::parse)
                .unwrap_or_else(crate::ui::deliver::saved_encoder);
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
                    .generate(
                        &request,
                        &flikk_core::paths::generated(),
                        &flikk_core::cloud::GenerationTask::default(),
                    )
                    .map_err(|e| anyhow!(generate::error_text(&e)))?;
                Ok(Generated {
                    item: flikk_core::cloud::media_item(&made, media.as_deref()),
                    destination: placement.into(),
                    summary: request.summary(),
                })
            })))
        }
        "generate_video" => {
            use flikk_agent::tools::video;
            let shot =
                video::from_json(args).ok_or_else(|| anyhow!("the request could not be read"))?;
            let generation = inputs
                .services
                .generation
                .clone()
                .ok_or_else(|| anyhow!("generation is not available"))?;
            shot.check(generation.video_model().as_ref())
                .map_err(|e| anyhow!(e))?;
            let services = inputs.services.clone();
            let project = inputs.project.clone();
            Ok(Started::Generate(Box::new(move || {
                let request = shot
                    .prepare(&project, &services, &|| false)
                    .map_err(|e| anyhow!(e))?;
                let made = generation
                    .generate(
                        &request,
                        &flikk_core::paths::generated(),
                        &flikk_core::cloud::GenerationTask::default(),
                    )
                    .map_err(|e| anyhow!(video::error_text(&e)))?;
                Ok(Generated {
                    item: flikk_core::cloud::media_item(&made, services.media.as_deref()),
                    destination: shot.placement.into(),
                    summary: shot.request.summary(),
                })
            })))
        }
        flikk_agent::tools::upscale::TOOL => {
            use flikk_agent::tools::upscale;
            let (request, destination) =
                upscale::from_json(args).ok_or_else(|| anyhow!("the request could not be read"))?;
            let generation = inputs
                .services
                .generation
                .clone()
                .ok_or_else(|| anyhow!("generation is not available"))?;
            let media = inputs.services.media.clone();
            Ok(Started::Generate(Box::new(move || {
                let made = generation
                    .generate(
                        &request,
                        &flikk_core::paths::generated(),
                        &flikk_core::cloud::GenerationTask::default(),
                    )
                    .map_err(|e| anyhow!(upscale::error_text(&e)))?;
                Ok(Generated {
                    item: flikk_core::cloud::media_item(&made, media.as_deref()),
                    destination,
                    summary: request.summary(),
                })
            })))
        }
        flikk_agent::tools::image::TOOL => {
            use flikk_agent::tools::image;
            let request =
                image::from_json(args).ok_or_else(|| anyhow!("the request could not be read"))?;
            let services = inputs.services.clone();
            let project = request.grabs_frames().then(|| inputs.project.clone());
            Ok(Started::Generate(Box::new(move || {
                let item = image::make(&request, project.as_ref(), &services, &Default::default())
                    .map_err(|e| anyhow!(e.0))?;
                Ok(Generated {
                    item,
                    destination: Destination::Pool,
                    summary: image::summary(&request),
                })
            })))
        }
        "replace_words" => {
            let (fix, reading) = flikk_agent::tools::respeak::from_json(args)
                .ok_or_else(|| anyhow!("the request could not be read"))?;
            let source = inputs
                .project
                .media(fix.media)
                .cloned()
                .ok_or_else(|| anyhow!("the recording is not in the pool"))?;
            let services = inputs.services.clone();
            Ok(Started::Respeak(Box::new(move || {
                let summary = flikk_agent::tools::respeak::summary(&fix, &reading);
                let task = flikk_core::cloud::GenerationTask::default();
                let item = flikk_core::respeak::make(&fix, &source, &reading, &services, &task)
                    .map_err(|e| anyhow!(e))?;
                Ok(Respoken { item, fix, summary })
            })))
        }
        "add_voice" => {
            let adding = flikk_agent::tools::voices::add_from_json(args)
                .ok_or_else(|| anyhow!("the request could not be read"))?;
            let (generation, media) = voice_services(inputs.services)?;
            Ok(Started::Voice(Box::new(move || {
                flikk_agent::tools::voices::add(generation.as_ref(), media.as_deref(), &adding)
                    .map_err(|e| anyhow!(e))
            })))
        }
        "design_voice" => {
            let (design, name) = flikk_agent::tools::voices::design_from_json(args)
                .ok_or_else(|| anyhow!("the request could not be read"))?;
            let (generation, media) = voice_services(inputs.services)?;
            Ok(Started::Voice(Box::new(move || {
                flikk_agent::tools::voices::design(
                    generation.as_ref(),
                    media.as_deref(),
                    &design,
                    &name,
                )
                .map_err(|e| anyhow!(e))
            })))
        }
        tool if flikk_agent::tools::caption_styles::TOOLS.contains(&tool) => {
            flikk_agent::tools::caption_styles::Op::from_json(args)
                .map(Started::CaptionStyle)
                .ok_or_else(|| anyhow!("the request could not be read"))
        }
        "export_otio" => {
            let project = inputs.project.clone();
            let path = free_output(&project, "otio");
            Ok(Started::Background(Box::new(move || {
                flikk_deliver::export_otio(&project, &path)?;
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
                flikk_deliver::export_frame(&project, t, &styles, &services, &path)?;
                Ok(file_name(&path))
            })))
        }
        other => bail!("`{other}` cannot be approved"),
    }
}

type VoiceServices = (
    std::sync::Arc<dyn flikk_core::cloud::GenerationService>,
    Option<std::sync::Arc<dyn flikk_core::services::MediaService>>,
);

fn voice_services(services: &Services) -> Result<VoiceServices> {
    let generation = services
        .generation
        .clone()
        .ok_or_else(|| anyhow!("generation is not available"))?;
    Ok((generation, services.media.clone()))
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
        status: RuleStatus::Approved,
        supersedes,
        created_at: flikk_store::now(),
        evidence: strings(args.get("evidence")),
    };
    // Saving it approved also retires the rule it supersedes, in one write.
    store(|s| s.save_rule(profile, &rule))?;
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

/// A skill or style the agent asked to save, as `tools::skills` wrote it
/// up for the card.
fn save_entry(args: &Value) -> Result<Started> {
    let text = |key| args.get(key).and_then(Value::as_str).unwrap_or_default();
    let draft = Draft {
        name: str_arg(args, "name")?.to_owned(),
        description: text("description").to_owned(),
        instructions: str_arg(args, "instructions")?.to_owned(),
        by_agent: args
            .get("by_agent")
            .and_then(Value::as_bool)
            .unwrap_or(true),
    };
    let previous = args.get("id").and_then(Value::as_str);
    skills::save(entry_kind(args), previous, &draft).map_err(|e| anyhow!(e))?;
    Ok(Started::Done("Saved".into()))
}

fn delete_entry(args: &Value) -> Result<Started> {
    skills::delete(entry_kind(args), str_arg(args, "id")?);
    Ok(Started::Done("Deleted".into()))
}

fn entry_kind(args: &Value) -> Kind {
    match args.get("kind").and_then(Value::as_str) {
        Some("style") => Kind::Style,
        _ => Kind::Skill,
    }
}

/// Save a project program to the user's library. The editor links the
/// project's copy to it afterwards (see `EditorState::approve`).
fn save_fx(args: &Value, project: &Project) -> Result<Started> {
    let text = str_arg(args, "fx")?;
    let asset = flikk_core::fx::find(project, text)
        .and_then(|r| flikk_core::fx::asset(project, &r))
        .ok_or_else(|| anyhow!("program {text} is no longer in the project"))?;
    let lib = flikk_core::library::Library::load();
    flikk_core::library::save_program(&lib, asset).map_err(|e| anyhow!(e))?;
    Ok(Started::Done("Saved to the library".into()))
}

/// Save audio from the pool to the user's library.
fn save_sound(args: &Value, project: &Project) -> Result<Started> {
    let text = str_arg(args, "media")?;
    let item = uuid::Uuid::parse_str(text)
        .ok()
        .and_then(|id| project.media(flikk_core::MediaId(id)))
        .ok_or_else(|| anyhow!("media {text} is no longer in the project"))?;
    let lib = flikk_core::library::Library::load();
    flikk_core::library::save_sound(&lib, item).map_err(|e| anyhow!(e))?;
    Ok(Started::Done("Saved to the library".into()))
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
            } else if key.contains("hevc") || key.contains("265") {
                RenderSettings::hevc_4k30()
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
    flikk_deliver::free_name(&output_dir().join(format!("{stem}.{ext}")))
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

fn store<T>(f: impl FnOnce(&flikk_store::Store) -> flikk_store::Result<T>) -> Result<T> {
    persistence::try_store(f).map_err(|e| anyhow!(e))
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
