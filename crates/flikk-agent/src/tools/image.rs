//! `generate_image`: a picture made with the user's ChatGPT plan, through
//! the image service Codex itself uses (`gpt-image-2`), signed in with
//! Codex's own ChatGPT login. Offered only when the user turns image
//! generation on, with either backend. Each picture counts against their
//! plan, so it waits for their approval unless they allowed generating. It
//! goes to the pool's Images Generated bin.
//!
//! The request is the one Codex sends (`codex-rs/ext/image-generation`):
//! the prompt, a fixed model, automatic size and quality, and an opaque or
//! transparent background. The service is ChatGPT's own, not the public
//! Images API, and free plans are refused.

use super::{Done, Job, JobDone, Step, ToolCtx};
use crate::args::{Args, ToolError, ToolResultOf, fail};
use base64::Engine;
use flikk_core::agent::AgentSettings;
use flikk_core::cloud;
use flikk_core::model::MediaItem;
use flikk_core::paths;
use flikk_core::services::MediaService;
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

/// What to make.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub prompt: String,
    pub transparent: bool,
}

pub fn parse(args: &Args) -> ToolResultOf<Request> {
    let prompt = args.str("prompt")?.trim().to_owned();
    if prompt.is_empty() {
        return fail("`prompt` describes the picture");
    }
    if prompt.chars().count() > MAX_PROMPT {
        return fail(format!("`prompt` runs at most {MAX_PROMPT} characters"));
    }
    Ok(Request {
        prompt,
        transparent: args.opt_bool("transparent")?.unwrap_or(false),
    })
}

/// One line for the approval card and the activity log.
pub fn summary(request: &Request) -> String {
    const SHOWN: usize = 80;
    let mut text: String = request.prompt.chars().take(SHOWN).collect();
    if request.prompt.chars().count() > SHOWN {
        text.push('…');
    }
    format!("Picture: {text}")
}

/// The request as an approval carries it.
pub fn request_json(request: &Request) -> Value {
    json!({ "prompt": request.prompt, "transparent": request.transparent })
}

/// What [`request_json`] made.
pub fn from_json(value: &Value) -> Option<Request> {
    Some(Request {
        prompt: value.get("prompt")?.as_str()?.to_owned(),
        transparent: value
            .get("transparent")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

/// The paid ChatGPT plan Codex is signed in with ("plus", "pro",
/// "team"…), if any: the account link `generate_image` needs. Free plans
/// have no image generation.
pub fn subscription() -> Option<String> {
    Login::load()?.paid().map(str::to_owned)
}

/// Whether turns are offered `generate_image`: the user turned it on and
/// Codex is signed in with a paid ChatGPT plan.
pub fn offered(settings: &AgentSettings) -> bool {
    settings.image_generation && subscription().is_some()
}

/// Why the call stops before anything is sent: image generation is off, or
/// there is no ChatGPT plan to make the picture with.
pub fn check(ctx: &ToolCtx) -> ToolResultOf<()> {
    if !ctx.settings.image_generation {
        return fail(
            "The user has not turned on image generation (Settings → Agent). Tell the user; don't retry.",
        );
    }
    Login::read().map(|_| ())
}

/// With the user's leave: a job that makes the picture and comes back to
/// the UI thread to add it to the pool.
pub fn serve(ctx: &mut ToolCtx, args: &Args) -> ToolResultOf<Step> {
    check(ctx)?;
    let request = parse(args)?;
    let media = ctx.services.media.clone();
    let job: Job = Box::new(move || match make(&request, media.as_deref()) {
        Ok(item) => JobDone::Generated {
            item: Box::new(item),
            placement: None,
            summary: summary(&request),
        },
        Err(ToolError(e)) => JobDone::Done(Done::error(e)),
    });
    Ok(Step::Job(job))
}

/// Make the picture and keep it with the user's data, as a pool item not
/// in a bin yet. Blocks for as long as the service takes.
pub fn make(request: &Request, media: Option<&dyn MediaService>) -> ToolResultOf<MediaItem> {
    let login = Login::read()?;
    let bytes = generate(&login, request)?;
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
fn generate(login: &Login, request: &Request) -> ToolResultOf<Vec<u8>> {
    use ureq::tls::{RootCerts, TlsConfig};
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(
            TlsConfig::builder()
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .user_agent(concat!("flikk/", env!("CARGO_PKG_VERSION")))
        .http_status_as_error(false)
        .timeout_global(Some(TIMEOUT))
        .build()
        .into();
    let body = json!({
        "prompt": request.prompt,
        "model": MODEL,
        "quality": "auto",
        "size": "auto",
        "background": if request.transparent { "transparent" } else { "opaque" },
    });
    let mut response = agent
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
