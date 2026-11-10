//! Writing and changing a blockout by asking. The user says what should be
//! different ("bring the camera lower", "the car comes in later"), or asks
//! for a blockout of a shot that has none; one backend run without tools
//! reads the scene, or the shot's description, and answers with the scene.
//! It costs what a short chat message costs, and nothing of the video
//! provider's.

use crate::backend::{self, BackendEvent, RunningTurn, TurnSpec};
use crate::session;
use flikk_core::agent::AgentSettings;
use flikk_core::blockout::Scene;
use std::path::PathBuf;

/// How many earlier asks are sent along, so "a bit more" has something to
/// refer to.
const REMEMBERED: usize = 6;

/// One thing the user asked for and what was answered.
#[derive(Clone, Debug, PartialEq)]
pub struct Exchange {
    pub ask: String,
    pub answer: String,
}

/// A change to a blockout being worked out. Dropping it stops it.
pub struct Reblock {
    running: RunningTurn,
    reply: String,
    run_dir: PathBuf,
}

/// Where a [`Reblock`] is.
#[derive(Clone, Debug, PartialEq)]
pub enum Reblocked {
    Working,
    /// The answer, and the scene changed; `None` when the answer is that
    /// it can't be done, or a question.
    Done {
        scene: Option<Scene>,
        answer: String,
    },
    Failed(String),
}

impl Reblock {
    /// Start changing `scene` as `ask` says, on the backend and model the
    /// user picked for the chat. `earlier` is what was asked before in the
    /// same sitting, oldest first.
    pub fn start(
        scene: &Scene,
        earlier: &[Exchange],
        ask: &str,
        settings: &AgentSettings,
    ) -> std::io::Result<Self> {
        Self::run(CHANGING, change_message(scene, earlier, ask), settings)
    }

    /// Start writing a blockout for the video `shot` describes, `seconds`
    /// long in a picture `shape` (width to height), on the backend and
    /// model the user picked for the chat.
    pub fn draft(
        shot: &str,
        seconds: f64,
        shape: (u32, u32),
        settings: &AgentSettings,
    ) -> std::io::Result<Self> {
        let message = format!(
            "The shot, {seconds:.0} s long, {} wide to {} high:\n\n{shot}",
            shape.0, shape.1
        );
        Self::run(DRAFTING, message, settings)
    }

    /// One backend run without tools that is told `task` and the scene's
    /// format, and given `message`.
    fn run(task: &str, message: String, settings: &AgentSettings) -> std::io::Result<Self> {
        let dir = session::agent_dir();
        let workdir = dir.join("work");
        let run_dir = dir
            .join("run")
            .join(uuid::Uuid::new_v4().simple().to_string());
        std::fs::create_dir_all(&workdir)?;
        std::fs::create_dir_all(&run_dir)?;
        let backend = settings.backend;
        let spec = TurnSpec {
            system_prompt: system_prompt(task),
            message,
            resume: None,
            model: settings.models.get(backend).clone(),
            mcp: None,
            web: false,
            workdir,
            run_dir: run_dir.clone(),
        };
        match backend::start(backend::adapter(backend), spec) {
            Ok(running) => Ok(Self {
                running,
                reply: String::new(),
                run_dir,
            }),
            Err(e) => {
                let _ = std::fs::remove_dir_all(&run_dir);
                Err(std::io::Error::new(
                    e.kind(),
                    format!("Could not start {}: {e}", backend.label()),
                ))
            }
        }
    }

    /// Where it is now. Once it is done or failed, drop it.
    pub fn poll(&mut self) -> Reblocked {
        loop {
            match self.running.events.try_recv() {
                Ok(BackendEvent::TextDelta(text)) => self.reply.push_str(&text),
                Ok(BackendEvent::Finished { error: Some(e) }) => return Reblocked::Failed(e),
                Ok(BackendEvent::Finished { error: None }) => return read_reply(&self.reply),
                Ok(_) => {}
                Err(std::sync::mpsc::TryRecvError::Empty) => return Reblocked::Working,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return Reblocked::Failed("The agent stopped before answering".into());
                }
            }
        }
    }
}

impl Drop for Reblock {
    fn drop(&mut self) {
        self.running.kill();
        let _ = std::fs::remove_dir_all(&self.run_dir);
    }
}

/// What a blockout is, to whoever writes or changes one.
const BLOCKOUT: &str = "a blockout for the Flikk video editor: a rough 3D scene of grey stand-ins \
    and a moving camera, which a video model will follow for its camera move, staging and timing";

/// What a run that changes a scene is asked to do.
const CHANGING: &str = "The user watches the blockout play and tells you what should be \
    different. You get the scene as JSON and what they ask. Answer with one short sentence \
    saying what you changed, in the language they write in, then the whole scene, changed, in \
    one ```json block. Change only what they ask for; keep the objects' names; keep it rough \
    and the moves physically plausible. When what they ask can't be shown in this format, or \
    you need to know more, say so in one or two sentences and give no JSON.";

/// What a run that writes a scene from a shot's description is asked to do.
const DRAFTING: &str = "You get the description of one video shot, how long it runs and its \
    shape. Block it: the few stand-ins that matter, each named for what it stands for, where \
    they are and how they move, and the camera move the description asks for, or one that \
    suits it. Keep it rough and the moves physically plausible, with everything that matters \
    in view at the start; the look is not yours to show. Answer with one short sentence saying \
    what you blocked, in the language of the description, then the scene in one ```json \
    block. When the description gives nothing to block, say so in one sentence and give no \
    JSON.";

fn system_prompt(task: &str) -> String {
    let format = crate::tools::catalog::blockout();
    format!("You work on {BLOCKOUT}.\n\n{task}\n\nThe scene's format, as a JSON schema:\n{format}")
}

fn change_message(scene: &Scene, earlier: &[Exchange], ask: &str) -> String {
    let mut text = String::new();
    let remembered = &earlier[earlier.len().saturating_sub(REMEMBERED)..];
    if !remembered.is_empty() {
        text.push_str("Asked before, oldest first:\n");
        for exchange in remembered {
            text.push_str(&format!("- \"{}\" → {}\n", exchange.ask, exchange.answer));
        }
        text.push('\n');
    }
    let scene = serde_json::to_string(scene).unwrap_or_default();
    text.push_str(&format!("The scene now:\n```json\n{scene}\n```\n\n{ask}"));
    text
}

/// The answer and the scene in a reply: the JSON is its outermost braces,
/// the answer the text before them.
fn read_reply(reply: &str) -> Reblocked {
    let braces = reply.find('{').zip(reply.rfind('}')).filter(|(a, b)| a < b);
    let Some((start, end)) = braces else {
        return match reply.trim() {
            "" => Reblocked::Failed("The agent gave no answer".into()),
            answer => Reblocked::Done {
                scene: None,
                answer: answer.to_owned(),
            },
        };
    };
    let json = serde_json::from_str(&reply[start..=end])
        .map_err(|e| e.to_string())
        .and_then(|json| Scene::parse(&json));
    let answer = reply[..start]
        .trim()
        .trim_end_matches("```json")
        .trim_end_matches("```")
        .trim();
    match json {
        Ok(scene) => Reblocked::Done {
            scene: Some(scene),
            answer: match answer {
                "" => "Changed".to_owned(),
                answer => answer.to_owned(),
            },
        },
        Err(e) => Reblocked::Failed(format!("The changed scene could not be read: {e}")),
    }
}
