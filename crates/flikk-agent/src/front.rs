//! The app's door for other apps: the MCP clients on this machine (Claude
//! Code, Claude Desktop, Codex…) that run `flikk mcp` and find the running
//! app through `bridge.json`.
//!
//! What a client asks is either for one of the open projects or for the
//! app. A project's tools are the editor's own, served by that project's
//! host ([`crate::AgentHost::serve_external`]): each takes `project`, and
//! goes to the project in the editor without it. The app's are the ones
//! here: listing the open projects, starting one with its own agent on a
//! message, messaging an agent, waiting for agents and renders, saving and
//! closing. So a client makes several videos at once by starting a project
//! for each, and renders and saves them when their agents are done.
//!
//! The app carries them out ([`Front::asked`]); this module is the
//! listener, the tools' schemas and reading their arguments.

use crate::args::{Args, ToolError, ToolResultOf, fail};
use crate::bridge::{BridgeServer, Incoming, IncomingKind, Origin, Reply, ToolResult};
use crate::tools::ToolSpec;
use flikk_core::Rate;
use flikk_core::command::ToolCategory;
use flikk_core::skills::{self, Kind};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;

/// The argument that names the open project a project's tool acts on.
pub(crate) const PROJECT: &str = "project";

const LIST: &str = "list_projects";
const START: &str = "start_project";
const MESSAGE: &str = "message_project";
const WAIT: &str = "wait_for_projects";
const SAVE: &str = "save_project";
const CLOSE: &str = "close_project";

/// The app's own tools.
const TOOLS: [&str; 6] = [LIST, START, MESSAGE, WAIT, SAVE, CLOSE];

/// How long `wait_for_projects` waits unless told: under the minute many
/// clients give a call.
const WAIT_DEFAULT: Duration = Duration::from_secs(45);
/// The longest it waits: a night's work. A client that lets a call run
/// this long (Claude Code moves it to the background, and its answer wakes
/// the model) is told when the projects are done rather than asking again.
const WAIT_MAX: Duration = Duration::from_secs(12 * 3600);

/// The least and most a sequence is wide or high.
const SIDE: std::ops::RangeInclusive<u32> = 128..=8192;

/// The listener other apps connect to, published while it lives.
pub struct Front {
    server: BridgeServer,
}

impl Front {
    /// Start listening and publish where in `bridge.json`.
    pub fn open() -> std::io::Result<Self> {
        let mut server = BridgeServer::start()?;
        server.publish()?;
        Ok(Self { server })
    }

    /// What clients asked since the last call; never blocks. A call whose
    /// arguments can't be read is answered here and left out.
    pub fn asked(&self) -> Vec<Asked> {
        let incoming = self.server.drain().into_iter();
        incoming.filter_map(read).collect()
    }
}

fn read(incoming: Incoming) -> Option<Asked> {
    let Incoming {
        origin,
        request,
        reply,
    } = incoming;
    // The turn token of this listener is handed to nobody.
    let Origin::External(client) = origin else {
        return None;
    };
    let (project, request) = match request {
        IncomingKind::Closed => return Some(Asked::Left(client)),
        IncomingKind::List => (None, IncomingKind::List),
        IncomingKind::Call { tool, args } if TOOLS.contains(&tool.as_str()) => {
            return match parse(&tool, &args) {
                Ok(request) => Some(Asked::App(AppCall {
                    request,
                    reply: AppReply { client, reply },
                })),
                Err(ToolError(message)) => {
                    reply.result(ToolResult::error(message));
                    None
                }
            };
        }
        IncomingKind::Call { tool, mut args } => {
            let named = args.as_object_mut().and_then(|a| a.remove(PROJECT));
            let project = named
                .as_ref()
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(str::to_owned);
            (project, IncomingKind::Call { tool, args })
        }
    };
    Some(Asked::Project(ProjectCall {
        project,
        incoming: Incoming {
            origin,
            request,
            reply,
        },
    }))
}

/// One thing a client asked.
pub enum Asked {
    /// A project's tool, or the list of tools: for the host of the project
    /// it names.
    Project(ProjectCall),
    /// One of the app's own tools.
    App(AppCall),
    /// The client with this id disconnected: nobody waits for what it
    /// asked any more.
    Left(u64),
}

/// A call for one open project.
pub struct ProjectCall {
    project: Option<String>,
    pub(crate) incoming: Incoming,
}

impl ProjectCall {
    /// The project it names, by id or name; `None` for the one in the
    /// editor.
    pub fn project(&self) -> Option<&str> {
        self.project.as_deref()
    }

    /// Answer that it can't be served, such as when no open project is the
    /// one it names.
    pub fn refuse(self, why: impl Into<String>) {
        self.incoming.reply.result(ToolResult::error(why));
    }
}

/// A call of one of the app's own tools, with where its answer goes.
pub struct AppCall {
    pub request: Request,
    pub reply: AppReply,
}

/// Answers one [`AppCall`]; dropped without an answer, the client waits
/// until it gives up.
pub struct AppReply {
    client: u64,
    reply: Reply,
}

impl AppReply {
    /// The id of the client that asked.
    pub fn client(&self) -> u64 {
        self.client
    }

    /// Tell the client the call is still under way, `waited` so far, and
    /// in `message` what it waits for. A client that asked for progress
    /// shows it, and does not take the silence for a call that hangs.
    pub fn progress(&self, waited: Duration, message: String) {
        // To the millisecond: it still grows from one report to the next.
        let seconds = waited.as_millis() as f64 / 1000.;
        self.reply.progress(seconds, message);
    }

    pub fn done(self, result: &Value) {
        self.reply.result(ToolResult::json(result));
    }

    pub fn failed(self, why: impl Into<String>) {
        self.reply.result(ToolResult::error(why));
    }
}

/// What one of the app's tools asks. Projects are named by id or name.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    /// The open projects and where each stands.
    List,
    Start(Start),
    /// Send `message` to the agent of `project`.
    Message {
        project: String,
        message: String,
    },
    /// Answer once none of `projects` (all the open ones when empty) has
    /// an agent or a render at work, or after `timeout`.
    Wait {
        projects: Vec<String>,
        timeout: Duration,
    },
    /// Save `project` (the one in the editor without) to its file.
    Save {
        project: Option<String>,
    },
    Close {
        project: String,
    },
}

impl Request {
    /// The permission that lets a client ask this; none for what only
    /// reads.
    pub fn category(&self) -> Option<ToolCategory> {
        match self {
            Self::List | Self::Wait { .. } => None,
            _ => Some(ToolCategory::Projects),
        }
    }
}

/// A project to make, and the message its agent starts on.
#[derive(Clone, Debug, PartialEq)]
pub struct Start {
    pub prompt: String,
    /// The project's name; from the prompt without one.
    pub name: Option<String>,
    /// Where its file goes; the user's folder for new projects without.
    pub folder: Option<PathBuf>,
    /// Files and folders to import, which the agent may also read.
    pub footage: Vec<PathBuf>,
    /// The id of the style the message is sent with.
    pub style: Option<String>,
    /// Width and height of the sequence.
    pub format: Option<(u32, u32)>,
    pub rate: Option<Rate>,
}

fn parse(tool: &str, args: &Value) -> ToolResultOf<Request> {
    let empty = serde_json::Map::new();
    // No argument here is a time: the rate is never read.
    let args = match args {
        Value::Null => Args(&empty, Rate::FPS_30),
        args => Args::of(args, Rate::FPS_30)?,
    };
    let named = |key| -> ToolResultOf<String> { Ok(args.str(key)?.trim().to_owned()) };
    Ok(match tool {
        LIST => Request::List,
        START => Request::Start(start(&args)?),
        MESSAGE => Request::Message {
            project: named(PROJECT)?,
            message: said(&args, "message")?,
        },
        WAIT => {
            let asked = args.opt_f64("timeout")?;
            let asked = asked.map(|s| s.clamp(1., WAIT_MAX.as_secs_f64()));
            Request::Wait {
                projects: args.strings("projects")?,
                timeout: asked.map_or(WAIT_DEFAULT, Duration::from_secs_f64),
            }
        }
        SAVE => Request::Save {
            project: args.opt_str(PROJECT)?.map(|p| p.trim().to_owned()),
        },
        CLOSE => Request::Close {
            project: named(PROJECT)?,
        },
        other => return fail(format!("unknown tool `{other}`")),
    })
}

/// The text under `key`, which an agent is sent: never empty.
fn said(args: &Args, key: &str) -> ToolResultOf<String> {
    let text = args.str(key)?.trim();
    if text.is_empty() {
        return fail(format!("`{key}` is empty"));
    }
    Ok(text.to_owned())
}

fn start(args: &Args) -> ToolResultOf<Start> {
    let absolute = |key: &str, text: &str| -> ToolResultOf<PathBuf> {
        let path = PathBuf::from(text.trim());
        if !path.is_absolute() {
            return fail(format!("`{key}` takes absolute paths, not `{text}`"));
        }
        Ok(path)
    };
    let footage = args.strings("footage")?;
    let footage: Vec<PathBuf> = (footage.iter())
        .map(|path| absolute("footage", path))
        .collect::<ToolResultOf<_>>()?;
    if let Some(missing) = footage.iter().find(|path| !path.exists()) {
        return fail(format!("`{}` does not exist", missing.display()));
    }
    let side = |key: &str| -> ToolResultOf<Option<u32>> {
        let Some(side) = args.opt_f64(key)? else {
            return Ok(None);
        };
        let side = side.round() as u32;
        if !SIDE.contains(&side) || !side.is_multiple_of(2) {
            let (least, most) = (SIDE.start(), SIDE.end());
            return fail(format!(
                "`{key}` is an even number of pixels from {least} to {most}"
            ));
        }
        Ok(Some(side))
    };
    let format = match (side("width")?, side("height")?) {
        (Some(width), Some(height)) => Some((width, height)),
        (None, None) => None,
        _ => return fail("give `width` and `height` together"),
    };
    Ok(Start {
        prompt: said(args, "prompt")?,
        name: (args.opt_str("name")?.map(str::trim))
            .filter(|name| !name.is_empty())
            .map(str::to_owned),
        folder: (args.opt_str("folder")?)
            .map(|folder| absolute("folder", folder))
            .transpose()?,
        footage,
        style: args.opt_str("style")?.map(style).transpose()?,
        format,
        rate: args.opt_f64("fps")?.map(rate).transpose()?,
    })
}

/// The id of the user's style called `name` (or with that id).
fn style(name: &str) -> ToolResultOf<String> {
    let styles = skills::list(Kind::Style);
    let wanted = name.trim().to_lowercase();
    let found = (styles.iter()).find(|s| s.id == wanted || s.name.to_lowercase() == wanted);
    match found {
        Some(style) => Ok(style.id.clone()),
        None if styles.is_empty() => fail("The user has no styles; leave `style` out."),
        None => {
            let names: Vec<_> = styles.iter().map(|s| s.name.as_str()).collect();
            fail(format!(
                "No style is called `{name}`. The user's styles: {}.",
                names.join(", ")
            ))
        }
    }
}

/// The sequence rate nearest to `fps`, when it is one a sequence takes.
fn rate(fps: f64) -> ToolResultOf<Rate> {
    let offered = Rate::SEQUENCE.iter();
    let found = offered
        .clone()
        .find(|(rate, _)| (rate.fps() - fps).abs() < 0.01);
    match found {
        Some((rate, _)) => Ok(*rate),
        None => {
            let names: Vec<_> = offered.map(|(_, name)| *name).collect();
            fail(format!("`fps` is one of {}", names.join(", ")))
        }
    }
}

/// The schemas of the app's tools, for a client's list.
pub(crate) fn specs() -> Vec<ToolSpec> {
    let text = |desc: &str| json!({ "type": "string", "description": desc });
    let project = || text("The project's id or name, from list_projects");
    let object = |props: Value, required: &[&str]| json!({ "type": "object", "properties": props, "required": required });
    let spec = |name, reads: bool, description, schema| ToolSpec {
        name,
        category: Some(if reads {
            ToolCategory::ReadProject
        } else {
            ToolCategory::Projects
        }),
        description,
        schema,
    };
    vec![
        spec(
            LIST,
            true,
            "The projects open in Flikk: each one's id, name, file, format and length, whether it has unsaved changes, where its agent stands (idle, working, waiting on the user, failed) with what it last said or asked, what waits for the user's review or approval, and its renders.",
            object(json!({}), &[]),
        ),
        spec(
            START,
            false,
            "Make a new project and put Flikk's own agent to work in it on `prompt`, as if the user had written it in that project's chat. Every project has an agent of its own and they work at the same time, so to make several videos call this once for each, then wait_for_projects. The agent edits with what the user allows it: where they are asked, its changes wait for their review in Flikk. Returns the project's id and file.",
            object(
                json!({
                    "prompt": text("What the agent is asked to make: the brief for this video, complete in itself"),
                    "name": text("The project's name, which is also its file's and its render's (default: the prompt's first words)"),
                    "folder": text("Absolute path of the folder the project file is saved in (default: the user's folder for new projects)"),
                    "footage": { "type": "array", "items": { "type": "string" }, "description": "Absolute paths of the files and folders to work from: media is imported into the project, and the agent may read them all" },
                    "style": text("One of the user's styles, by name: the agent follows it"),
                    "width": { "type": "number", "description": "Sequence width in pixels, with `height` (default 1920 by 1080)" },
                    "height": { "type": "number", "description": "Sequence height in pixels, with `width`: 1080 by 1920 for a vertical video" },
                    "fps": { "type": "number", "description": "Sequence frame rate: 23.976, 24, 25, 29.97, 30, 50 or 60 (default 60)" }
                }),
                &["prompt"],
            ),
        ),
        spec(
            MESSAGE,
            false,
            "Send a project's agent another message, as the user would in its chat: a correction, the next step, or the answer to what it asked. Refused while that agent is working.",
            object(
                json!({ "project": project(), "message": text("The message") }),
                &["project", "message"],
            ),
        ),
        spec(
            WAIT,
            true,
            "Wait until no agent and no render of the projects is at work, then return them as list_projects does: call it once after starting projects or renders, rather than list_projects again and again. Returns after `timeout` seconds at the latest, with what is still at work: call it again then. Give a long `timeout` (7200 for two hours) when your client lets a call run that long, as Claude Code does: the call goes on in the background and its answer comes when the work is done.",
            object(
                json!({
                    "projects": { "type": "array", "items": { "type": "string" }, "description": "Ids or names (default: every open project)" },
                    "timeout": { "type": "number", "description": "Seconds to wait at most (default 45, for clients that give up on a call after a minute; at most 43200)" }
                }),
                &[],
            ),
        ),
        spec(
            SAVE,
            false,
            "Save a project to its file.",
            object(
                json!({ "project": text("The project's id or name (default: the one in the editor)") }),
                &[],
            ),
        ),
        spec(
            CLOSE,
            false,
            "Close an open project: its agent stops. Refused while it has unsaved changes: save_project first.",
            object(json!({ "project": project() }), &["project"]),
        ),
    ]
}
