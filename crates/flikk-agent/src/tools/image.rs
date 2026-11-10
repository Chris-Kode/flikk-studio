//! `generate_image`: a picture, from a description alone or from pictures
//! it edits or follows (pictures in the pool, or the program at a time).
//! It is made by the image provider set in Settings → Integrations when
//! there is one, whatever the agent's own setting says; else with the
//! user's ChatGPT plan, when they turned image generation on (Settings →
//! Agent). Each picture costs the user money or counts against their plan,
//! so each call waits for their approval. It goes to the pool's Images
//! Generated bin.
//!
//! The ChatGPT plan's pictures come from the image service Codex itself
//! uses (`gpt-image-2`), signed in with Codex's own ChatGPT login, as Codex
//! asks for them (`codex-rs/ext/image-generation`): the prompt, a fixed
//! model, automatic size and quality, and an opaque or transparent
//! background. The service is ChatGPT's own, not the public Images API, and
//! free plans are refused. It makes pictures from a description only.

use super::video::{self, FrameGrab};
use crate::args::{Args, ToolError, ToolResultOf, fail, media_id, parse_time};
use base64::Engine;
use flikk_core::agent::AgentSettings;
use flikk_core::cloud::{self, Capability, CloudError, GenerateRequest, GenerationTask};
use flikk_core::model::{MediaItem, MediaKind, Project};
use flikk_core::paths;
use flikk_core::services::{MediaService, Services};
use image::ImageFormat;
use serde_json::{Value, json};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const TOOL: &str = "generate_image";

const URL: &str = "https://chatgpt.com/backend-api/codex/images/generations";
const MODEL: &str = "gpt-image-2";
/// A picture takes up to a couple of minutes.
const TIMEOUT: Duration = Duration::from_secs(300);
/// Biggest answer read (the picture comes as base64 inside it).
const MAX_BYTES: u64 = 64 << 20;
/// Longest prompt sent.
const MAX_PROMPT: usize = 32_000;

/// Who makes the picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Route {
    /// The image provider set in Settings → Integrations.
    Provider,
    /// The user's ChatGPT plan, through Codex's sign-in.
    ChatGpt,
}

/// Who makes pictures for `settings`: the provider set for them, which
/// wins over the agent's own setting, else the ChatGPT plan when the user
/// turned it on and Codex is signed in with a paid one.
pub(crate) fn route(settings: &AgentSettings, services: &Services) -> Option<Route> {
    let provider = services
        .generation
        .as_ref()
        .is_some_and(|g| g.provider_for(Capability::Image).is_some());
    if provider {
        Some(Route::Provider)
    } else if settings.image_generation && subscription().is_some() {
        Some(Route::ChatGpt)
    } else {
        None
    }
}

/// Whether turns are offered `generate_image`.
pub(crate) fn offered(settings: &AgentSettings, services: &Services) -> bool {
    route(settings, services).is_some()
}

/// What to make with the ChatGPT plan.
#[derive(Clone, Debug, PartialEq)]
pub struct Prompt {
    pub prompt: String,
    pub transparent: bool,
}

/// What to make, and who makes it.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    /// By the provider, after grabbing the program's frames it follows.
    Provider {
        request: GenerateRequest,
        grabs: Vec<FrameGrab>,
    },
    ChatGpt(Prompt),
}

impl Request {
    /// Whether the program's frames are grabbed first.
    pub fn grabs_frames(&self) -> bool {
        matches!(self, Self::Provider { grabs, .. } if !grabs.is_empty())
    }
}

/// The request from the tool's arguments, for whoever makes pictures now;
/// a provider's is checked against the model the user picked.
pub(crate) fn parse(
    project: &Project,
    settings: &AgentSettings,
    services: &Services,
    args: &Args,
) -> ToolResultOf<Request> {
    let prompt = args.str("prompt")?.trim().to_owned();
    if prompt.is_empty() {
        return fail("`prompt` describes the picture");
    }
    if prompt.chars().count() > MAX_PROMPT {
        return fail(format!("`prompt` runs at most {MAX_PROMPT} characters"));
    }
    let transparent = args.opt_bool("transparent")?.unwrap_or(false);
    let Some(route) = route(settings, services) else {
        return fail(
            "No image provider is set in Settings → Integrations, and image generation with the ChatGPT plan is off (Settings → Agent). Tell the user; don't retry.",
        );
    };
    let mut references = Vec::new();
    for id in args.strings("references")? {
        let item = project
            .media(media_id(project, &id)?)
            .filter(|m| m.kind == MediaKind::Image);
        match item {
            Some(item) => references.push(item.path.clone()),
            None => return fail(format!("`references`: {id} is not a picture in the pool")),
        }
    }
    let mut grabs = Vec::new();
    if let Some(times) = args.value("frames_at") {
        let Some(times) = times.as_array() else {
            return fail("`frames_at` is a list of times");
        };
        for t in times {
            let grab = FrameGrab::new(parse_time(t, args.1)?);
            references.push(grab.path.clone());
            grabs.push(grab);
        }
    }
    match route {
        Route::ChatGpt => {
            if !references.is_empty() {
                return fail(
                    "Editing or following pictures needs an image provider set in Settings → Integrations; the ChatGPT plan makes pictures from a description only. Describe the picture instead, or tell the user.",
                );
            }
            Login::read()?;
            Ok(Request::ChatGpt(Prompt {
                prompt,
                transparent,
            }))
        }
        Route::Provider => {
            let request = GenerateRequest::Image {
                prompt,
                aspect: args.opt_str("aspect")?.map(str::to_owned),
                resolution: args.opt_str("resolution")?.map(str::to_owned),
                references,
                transparent,
            };
            if let Some(model) = services.generation.as_ref().and_then(|g| g.image_model()) {
                model
                    .check(&request)
                    .map_err(|e| ToolError(error_text(&e)))?;
            }
            Ok(Request::Provider { request, grabs })
        }
    }
}

/// A provider's refusal as the model should read it.
pub(crate) fn error_text(e: &CloudError) -> String {
    match e {
        CloudError::BadPrompt { message, .. } => format!(
            "Refused: {message}. Change the request to fit the model the user picked, or tell the user."
        ),
        e => super::generate::error_text(e),
    }
}

/// One line for the approval card and the activity log.
pub fn summary(request: &Request) -> String {
    const SHOWN: usize = 80;
    let prompt = match request {
        Request::Provider { request, .. } => request.prompt(),
        Request::ChatGpt(p) => &p.prompt,
    };
    let references = match request {
        Request::Provider {
            request: GenerateRequest::Image { references, .. },
            ..
        } => references.len(),
        _ => 0,
    };
    let mut text: String = prompt.chars().take(SHOWN).collect();
    if prompt.chars().count() > SHOWN {
        text.push('…');
    }
    match references {
        0 => format!("Picture: {text}"),
        1 => format!("Picture: {text} (from 1 picture)"),
        n => format!("Picture: {text} (from {n} pictures)"),
    }
}

/// The request as an approval carries it.
pub(crate) fn request_json(request: &Request) -> Value {
    match request {
        Request::Provider { request, grabs } => json!({
            "via": "provider",
            "request": request,
            "frames": grabs.iter().map(FrameGrab::to_json).collect::<Vec<_>>(),
        }),
        Request::ChatGpt(p) => json!({ "prompt": p.prompt, "transparent": p.transparent }),
    }
}

/// What [`request_json`] made.
pub fn from_json(value: &Value) -> Option<Request> {
    if value.get("via").and_then(Value::as_str) == Some("provider") {
        let request = serde_json::from_value(value.get("request")?.clone()).ok()?;
        let grabs = value
            .get("frames")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(FrameGrab::from_json).collect())
            .unwrap_or_default();
        return Some(Request::Provider { request, grabs });
    }
    Some(Request::ChatGpt(Prompt {
        prompt: value.get("prompt")?.as_str()?.to_owned(),
        transparent: value
            .get("transparent")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }))
}

/// The paid ChatGPT plan Codex is signed in with ("plus", "pro",
/// "team"…), if any: the account link the ChatGPT route needs. Free plans
/// have no image generation.
pub fn subscription() -> Option<String> {
    Login::load()?.paid().map(str::to_owned)
}

/// Make the picture and keep it with the user's data, as a pool item not
/// in a bin yet: grabbing the frames it follows from `project` first. Blocks
/// for as long as the provider or service takes.
pub fn make(
    request: &Request,
    project: Option<&Project>,
    services: &Services,
    task: &GenerationTask,
) -> ToolResultOf<MediaItem> {
    let (request, grabs) = match request {
        Request::Provider { request, grabs } => (request, grabs),
        Request::ChatGpt(prompt) => return chatgpt(prompt, services.media.as_deref()),
    };
    let Some(generation) = services.generation.as_ref() else {
        return fail("Generation is not available in this build.");
    };
    if !grabs.is_empty() {
        let Some(project) = project else {
            return fail("The frames to follow could not be grabbed.");
        };
        video::grab_all(project, services, grabs).map_err(ToolError)?;
    }
    let made = generation
        .generate(request, &paths::generated(), task)
        .map_err(|e| ToolError(error_text(&e)))?;
    Ok(cloud::media_item(&made, services.media.as_deref()))
}

/// Make the picture with the ChatGPT plan.
fn chatgpt(prompt: &Prompt, media: Option<&dyn MediaService>) -> ToolResultOf<MediaItem> {
    let login = Login::read()?;
    let bytes = generate(&login, prompt)?;
    let ext = match image::guess_format(&bytes) {
        Ok(ImageFormat::Png) => "png",
        Ok(ImageFormat::Jpeg) => "jpg",
        Ok(ImageFormat::WebP) => "webp",
        _ => return fail("The image service sent something that is not a picture."),
    };
    let dir = paths::generated();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("image-{}.{ext}", uuid::Uuid::new_v4().simple()));
    std::fs::write(&path, &bytes)?;
    Ok(cloud::image_item(path, media))
}

/// Codex's ChatGPT sign-in, as `codex login` saved it.
struct Login {
    token: String,
    account: String,
    /// The ChatGPT plan, as the sign-in last saw it.
    plan: Option<String>,
    /// When the token expires, in Unix seconds.
    expires: Option<u64>,
}

impl Login {
    /// The saved sign-in, whatever its plan and age.
    fn load() -> Option<Self> {
        let text = std::fs::read_to_string(crate::backend::codex_home().join("auth.json")).ok()?;
        let auth: Value = serde_json::from_str(&text).ok()?;
        let tokens = auth.get("tokens")?;
        let token = tokens
            .get("access_token")?
            .as_str()
            .filter(|t| !t.is_empty())?;
        let access = jwt_claims(token);
        let id = tokens
            .get("id_token")
            .and_then(Value::as_str)
            .and_then(jwt_claims);
        let claim = |key: &str| {
            [&id, &access].into_iter().flatten().find_map(|c| {
                c.get("https://api.openai.com/auth")?
                    .get(key)?
                    .as_str()
                    .map(str::to_owned)
            })
        };
        let account = tokens
            .get("account_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| claim("chatgpt_account_id"))?;
        Some(Self {
            token: token.to_owned(),
            account,
            plan: claim("chatgpt_plan_type"),
            expires: access.as_ref().and_then(|c| c.get("exp")?.as_u64()),
        })
    }

    /// The plan, unless it is free (or unknown).
    fn paid(&self) -> Option<&str> {
        self.plan
            .as_deref()
            .filter(|p| !p.is_empty() && !p.eq_ignore_ascii_case("free"))
    }

    /// The sign-in, if a picture can be asked for with it.
    fn read() -> ToolResultOf<Self> {
        let Some(login) = Self::load() else {
            return fail(
                "Codex is not signed in with ChatGPT on this computer, which image generation needs. Tell the user to run `codex login` and sign in with ChatGPT; don't retry.",
            );
        };
        if login.paid().is_none() {
            return fail(
                "The ChatGPT plan Codex is signed in with does not include image generation (free plans don't). Tell the user; don't retry.",
            );
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        if login.expires.is_some_and(|exp| exp <= now + 60) {
            return fail(
                "The Codex sign-in has expired. Tell the user to open Codex once (or run `codex login`) to renew it; don't retry.",
            );
        }
        Ok(login)
    }
}

/// The claims of a JWT, unverified: only to read the account and when it
/// expires. The service checks the token itself.
fn jwt_claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// The picture's bytes.
fn generate(login: &Login, request: &Prompt) -> ToolResultOf<Vec<u8>> {
    let body = json!({
        "prompt": request.prompt,
        "model": MODEL,
        "quality": "auto",
        "size": "auto",
        "background": if request.transparent { "transparent" } else { "opaque" },
    });
    let mut response = super::web::http_agent(TIMEOUT)
        .post(URL)
        .header("Authorization", format!("Bearer {}", login.token))
        .header("ChatGPT-Account-ID", &login.account)
        .header("originator", "flikk")
        .header("Content-Type", "application/json")
        .send(body.to_string())
        .map_err(|e| ToolError(format!("Could not reach the image service: {e}")))?;
    let status = response.status().as_u16();
    let text = response
        .body_mut()
        .with_config()
        .limit(MAX_BYTES)
        .read_to_string()
        .map_err(|e| ToolError(format!("Could not read the picture: {e}")))?;
    let answer: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if !(200..300).contains(&status) {
        return Err(ToolError(refusal(status, &answer)));
    }
    let data = answer
        .pointer("/data/0/b64_json")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError("The image service sent no picture.".into()))?;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|_| ToolError("The image service sent a picture that could not be read.".into()))
}

/// A refusal as the model should read it.
fn refusal(status: u16, answer: &Value) -> String {
    let detail = answer
        .pointer("/error/message")
        .or_else(|| answer.get("detail"))
        .and_then(Value::as_str)
        .filter(|d| !d.is_empty() && *d != "Forbidden");
    match (status, detail) {
        (401, _) => "ChatGPT refused the Codex sign-in. Tell the user to run `codex login` again; don't retry.".into(),
        (403, _) => "The ChatGPT plan Codex is signed in with does not include image generation (free plans don't). Tell the user; don't retry.".into(),
        (429, _) => "The ChatGPT plan's image limit is reached for now. Tell the user; don't retry.".into(),
        (400, Some(d)) => format!("Refused: {d}. Change the prompt, or tell the user."),
        (_, Some(d)) => format!("The image service answered {status}: {d}"),
        (_, None) => format!("The image service answered {status}."),
    }
}
