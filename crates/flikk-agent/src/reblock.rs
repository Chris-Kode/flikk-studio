//! Writing and changing a blockout by asking. The user says what should be
//! different ("bring the camera lower", "the car comes in later"), or asks
//! for a blockout of a shot that has none; a backend run without tools
//! reads the scene, or the shot's description, and answers with the scene.
//! The run can't watch what it writes, so it is told in words what the
//! camera sees ([`Scene::review`]), and a first draft that looks mistaken
//! is handed back to it once to put right. It costs what a short chat
//! message costs, and nothing of the video provider's.

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
    task: &'static str,
    /// What the run was given, and whom it runs as.
    brief: String,
    settings: AgentSettings,
    /// The picture's shape, width to height.
    shape: (u32, u32),
    /// A first draft handed back to be put right: what stands if the
    /// second answer has no scene.
    draft: Option<(Box<Scene>, String)>,
    /// Whether a draft that looks mistaken is handed back.
    checks: bool,
}

/// Where a [`Reblock`] is.
#[derive(Clone, Debug, PartialEq)]
pub enum Reblocked {
    Working,
    /// The answer, and the scene changed; `None` when the answer is that
    /// it can't be done, or a question.
    Done {
        scene: Option<Box<Scene>>,
        answer: String,
    },
    Failed(String),
}

impl Reblock {
    /// Start changing `scene`, shown in a picture `shape` (width to
    /// height), as `ask` says, on the backend and model the user picked
    /// for the chat. `earlier` is what was asked before in the same
    /// sitting, oldest first.
    pub fn start(
        scene: &Scene,
        shape: (u32, u32),
        earlier: &[Exchange],
        ask: &str,
        settings: &AgentSettings,
    ) -> std::io::Result<Self> {
        let brief = change_message(scene, shape, earlier, ask);
        Self::run(CHANGING, brief, shape, false, settings)
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
        let brief = format!(
            "The shot, {seconds:.0} s long, {} wide to {} high:\n\n{shot}",
            shape.0, shape.1
        );
        Self::run(DRAFTING, brief, shape, true, settings)
    }

    fn run(
        task: &'static str,
        brief: String,
        shape: (u32, u32),
        checks: bool,
        settings: &AgentSettings,
    ) -> std::io::Result<Self> {
        let (running, run_dir) = launch(task, brief.clone(), settings)?;
        Ok(Self {
            running,
            reply: String::new(),
            run_dir,
            task,
            brief,
            settings: settings.clone(),
            shape,
            draft: None,
            checks,
        })
    }

    /// Where it is now. Once it is done or failed, drop it.
    pub fn poll(&mut self) -> Reblocked {
        loop {
            match self.running.events.try_recv() {
                Ok(BackendEvent::TextDelta(text)) => self.reply.push_str(&text),
                Ok(BackendEvent::Finished { error }) => return self.finished(error),
                Ok(_) => {}
                Err(std::sync::mpsc::TryRecvError::Empty) => return Reblocked::Working,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return self.finished(Some("The agent stopped before answering".into()));
                }
            }
        }
    }

    /// What a run that ended, with `error` or with its reply, leaves.
    fn finished(&mut self, error: Option<String>) -> Reblocked {
        let read = match error {
            Some(e) => Reblocked::Failed(e),
            None => read_reply(&self.reply),
        };
        match (read, self.draft.take()) {
            // The second look came to nothing: the first draft stands.
            (Reblocked::Failed(_) | Reblocked::Done { scene: None, .. }, Some((scene, answer))) => {
                Reblocked::Done {
                    scene: Some(scene),
                    answer,
                }
            }
            (
                Reblocked::Done {
                    scene: Some(scene),
                    answer,
                },
                None,
            ) if self.checks => self.check(scene, answer),
            (read, _) => read,
        }
    }

    /// Hand a first draft that looks mistaken back to be put right; one
    /// that looks sound is the answer.
    fn check(&mut self, scene: Box<Scene>, answer: String) -> Reblocked {
        self.checks = false;
        let review = scene.review(self.shape);
        let done = |scene, answer| Reblocked::Done {
            scene: Some(scene),
            answer,
        };
        if review.doubts.is_empty() {
            return done(scene, answer);
        }
        let written = serde_json::to_string(&scene).unwrap_or_default();
        let message = format!(
            "{}\n\nYou answered with this scene:\n```json\n{written}\n```\n\nFlikk drew it. What \
             its camera sees:\n{}\n\nPut right what is not as the shot needs, and answer again the \
             same way with the whole scene. What is meant (a subject hidden for a reveal) stays.",
            self.brief,
            review.text()
        );
        match launch(self.task, message, &self.settings) {
            Ok((running, run_dir)) => {
                self.running.kill();
                let _ = std::fs::remove_dir_all(&self.run_dir);
                (self.running, self.run_dir) = (running, run_dir);
                self.reply.clear();
                self.draft = Some((scene, answer));
                Reblocked::Working
            }
            Err(_) => done(scene, answer),
        }
    }
}

/// Start one backend run without tools that is told `task` and the
/// scene's format, and given `message`; with it, the folder to remove
/// when it is over.
fn launch(
    task: &str,
    message: String,
    settings: &AgentSettings,
) -> std::io::Result<(RunningTurn, PathBuf)> {
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
        Ok(running) => Ok((running, run_dir)),
        Err(e) => {
            let _ = std::fs::remove_dir_all(&run_dir);
            Err(std::io::Error::new(
                e.kind(),
                format!("Could not start {}: {e}", backend.label()),
            ))
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
const BLOCKOUT: &str = "a blockout for the Flikk video editor: a rough 3D scene of grey stand-ins, \
    built and moved by you, and a moving camera, which a video model will follow for its camera \
    move, staging, action and timing";

/// How a scene is written well, whoever writes it.
const CRAFT: &str = "Write it as a director blocks a shot. Design the characters and things the \
    shot is about yourself, as models of a few solids with joints where they bend, and give them \
    the motion the description asks for: a gait of their own by the ground they cover, a gesture \
    in keys, a breath or a sway by the clock. Give whatever travels `\"face\": \"path\"` and keys \
    for where it goes and when (a walk is about 1.4 m/s, a run 3 to 6). Build the set too: the \
    walls, the street, the furniture the action touches or the camera passes, enough for depth \
    and for the move to read against, in shades that tell the subject from the set: characters \
    light (0.75 or more) so their limbs read, the set darker. Time the beats with keys, ease the \
    moves that start or stop, and give the camera one clear idea a shot: a dolly, an orbit, a \
    crane, a camera that rides its subject (`parent`), hand-held (`shake`), or cuts between \
    set-ups (`ease` \"cut\"). Keep sizes, speeds and moves physically plausible, and everything \
    that matters in view at the start. The look is not yours to show.";

/// What a run that changes a scene is asked to do.
const CHANGING: &str = "The user watches the blockout play and tells you what should be \
    different. You get the scene as JSON, what its camera sees in words (you can't watch it), \
    and what they ask. Answer with one short sentence saying what you changed, in the language \
    they write in, then the whole scene, changed, in one ```json block. Change only what they \
    ask for and keep the names of objects, models and parts. When what they ask can't be shown in this format, or \
    you need to know more, say so in one or two sentences and give no JSON.";

/// What a run that writes a scene from a shot's description is asked to do.
const DRAFTING: &str = "You get the description of one video shot, how long it runs and its \
    shape. Block it: the people and things that matter, each named for what it stands for, \
    where they are, what they do and how they move, the set around them, and the camera move \
    the description asks for, or one that suits it. Answer with one short sentence saying what \
    you blocked, in the language of the description, then the scene in one ```json block. When \
    the description gives nothing to block, say so in one sentence and give no JSON.";

fn system_prompt(task: &str) -> String {
    let format = crate::tools::catalog::blockout();
    format!(
        "You work on {BLOCKOUT}.\n\n{task}\n\n{CRAFT}\n\nThe scene's format, as a JSON \
         schema:\n{format}"
    )
}

fn change_message(scene: &Scene, shape: (u32, u32), earlier: &[Exchange], ask: &str) -> String {
    let mut text = String::new();
    let remembered = &earlier[earlier.len().saturating_sub(REMEMBERED)..];
    if !remembered.is_empty() {
        text.push_str("Asked before, oldest first:\n");
        for exchange in remembered {
            text.push_str(&format!("- \"{}\" → {}\n", exchange.ask, exchange.answer));
        }
        text.push('\n');
    }
    let written = serde_json::to_string(scene).unwrap_or_default();
    let seen = scene.review(shape).text();
    text.push_str(&format!(
        "The scene now:\n```json\n{written}\n```\n\nWhat its camera sees:\n{seen}\n\n{ask}"
    ));
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
            scene: Some(Box::new(scene)),
            answer: match answer {
                "" => "Changed".to_owned(),
                answer => answer.to_owned(),
            },
        },
        Err(e) => Reblocked::Failed(format!("The changed scene could not be read: {e}")),
    }
}
