//! What other apps ask of Flikk over MCP (`flikk_agent::front`). A call of
//! a project's tool goes to the agent link of the open project it names.
//! The app's own tools are carried out here: listing the open projects,
//! starting one with its agent on a message, messaging an agent, waiting
//! for agents and renders, saving and closing.
//!
//! Those that change something follow the user's setting for
//! `ToolCategory::Projects`: refused, carried out, or asked first, when
//! they wait in [`Desk::asks`] for the window's question.

use super::Desk;
use crate::persistence;
use crate::state::{AgentStatus, EditorState};
use flikk_agent::front::{AppCall, AppReply, Asked, Front, Request, Start};
use flikk_core::agent::ChatRole;
use flikk_core::bus::Permission;
use flikk_core::changeset::ChangeSetState;
use flikk_core::command::ToolCategory;
use flikk_core::model::SequenceSettings;
use flikk_deliver::RenderState;
use flikk_store::project_file::{EXTENSION, sanitize};
use gpui_kit::*;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How often what other apps asked is looked at.
const SERVE_EVERY: Duration = Duration::from_millis(50);

/// The most of an agent's last answer a client is told, from its end.
const SAID_MAX: usize = 2000;

/// The most of a prompt or message the question shows.
const DETAIL_MAX: usize = 140;

/// The door other apps come in by, and what they asked that is not
/// answered yet.
pub(super) struct Clients {
    /// None when it could not be opened: no other app connects.
    front: Option<Front>,
    asks: Vec<Ask>,
    waits: Vec<Wait>,
    /// The projects made for an agent whose first message is not sent
    /// yet: their footage is still being brought in.
    briefing: Vec<EntityId>,
    _serving: Task<()>,
}

/// Something another app asked that waits for the user's answer.
pub struct Ask {
    request: Request,
    reply: AppReply,
    /// What it would do: `Start “Lisbon teaser”`.
    pub title: String,
    /// The prompt or message it would send.
    pub detail: Option<String>,
}

/// A `wait_for_projects` not answered yet.
struct Wait {
    projects: Vec<Entity<EditorState>>,
    until: Instant,
    reply: AppReply,
}

impl Clients {
    /// Open the door and look at what comes in for as long as the desk
    /// lives.
    pub(super) fn open(window: &mut Window, cx: &mut Context<Desk>) -> Self {
        let front = Front::open()
            .inspect_err(|e| eprintln!("flikk: other apps can't connect: {e}"))
            .ok();
        let serving = cx.spawn_in(window, async move |desk, cx| {
            loop {
                cx.background_executor().timer(SERVE_EVERY).await;
                let served = desk.update_in(cx, |desk, window, cx| desk.serve_clients(window, cx));
                if served.is_err() {
                    break;
                }
            }
        });
        Self {
            front,
            asks: Vec::new(),
            waits: Vec::new(),
            briefing: Vec::new(),
            _serving: serving,
        }
    }

    /// `state`'s project was made for an agent whose message is on its
    /// way: it counts as working until [`Self::briefed`].
    pub(super) fn briefing(&mut self, state: &Entity<EditorState>) {
        self.briefing.push(state.entity_id());
    }

    pub(super) fn briefed(&mut self, state: &Entity<EditorState>) {
        self.briefing.retain(|id| *id != state.entity_id());
    }
}

impl Desk {
    /// What other apps asked that waits for the user's answer.
    pub fn asks(&self) -> &[Ask] {
        &self.clients.asks
    }

    /// The user's answer to everything in [`Self::asks`]: carry it out, or
    /// tell the apps it was denied.
    pub fn answer_asks(&mut self, accepted: bool, window: &mut Window, cx: &mut Context<Self>) {
        for ask in std::mem::take(&mut self.clients.asks) {
            if accepted {
                self.carry_out(ask.request, ask.reply, window, cx);
            } else {
                ask.reply
                    .failed(format!("Denied by the user: {}", ask.title));
            }
        }
        cx.notify();
    }

    /// Accept everything in [`Self::asks`], and whatever other apps ask
    /// from now on, until the user sets it back in Settings.
    pub fn always_accept(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |s, cx| {
            let mut settings = s.settings.clone();
            settings
                .permissions
                .set(ToolCategory::Projects, Permission::Accept);
            s.set_settings(settings, cx);
        });
        self.answer_asks(true, window, cx);
    }

    /// Take in what other apps asked since the last look, and answer the
    /// waits that are over.
    fn serve_clients(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let asked = match &self.clients.front {
            Some(front) => front.asked(),
            None => return,
        };
        for asked in asked {
            match asked {
                Asked::Project(call) => match self.named(call.project(), cx) {
                    Ok(state) => state.update(cx, |s, _| s.serve_external(call)),
                    Err(why) => call.refuse(why),
                },
                Asked::App(AppCall { request, reply }) => self.take(request, reply, window, cx),
                Asked::Left(client) => self.client_left(client, cx),
            }
        }
        self.end_waits(cx);
    }

    /// An app disconnected: nothing it asked is waited for any more.
    fn client_left(&mut self, client: u64, cx: &mut Context<Self>) {
        for state in self.states().cloned().collect::<Vec<_>>() {
            state.update(cx, |s, _| s.external_left(client));
        }
        let asks = self.clients.asks.len();
        self.clients.asks.retain(|a| a.reply.client() != client);
        self.clients.waits.retain(|w| w.reply.client() != client);
        if self.clients.asks.len() != asks {
            cx.notify();
        }
    }

    /// One of the app's tools was called: carry it out, ask the user
    /// first, or refuse it, as they set. What could not be done anyway is
    /// refused without asking.
    fn take(
        &mut self,
        request: Request,
        reply: AppReply,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(category) = request.category() else {
            return self.carry_out(request, reply, window, cx);
        };
        let ask = match self.describe(&request, cx) {
            Ok(ask) => ask,
            Err(why) => return reply.failed(why),
        };
        match self.editor.read(cx).bus.permissions().get(category) {
            Permission::Off => reply.failed(format!(
                "The user turned off \"{}\" in Flikk's settings.",
                category.label()
            )),
            Permission::Accept => self.carry_out(request, reply, window, cx),
            _ => {
                let (title, detail) = ask;
                self.clients.asks.push(Ask {
                    request,
                    reply,
                    title,
                    detail,
                });
                cx.notify();
            }
        }
    }

    /// What `request` would do, for the question: its line, and the text
    /// it would send. Why it can't be done, when it can't.
    fn describe(&self, request: &Request, cx: &App) -> Result<(String, Option<String>), String> {
        let name = |state: &Entity<EditorState>| state.read(cx).bus.project().name.clone();
        Ok(match request {
            Request::List | Request::Wait { .. } => (String::new(), None),
            Request::Start(start) => {
                if self.editor.read(cx).projects_wait() {
                    return Err("Flikk is still downloading its models; try again later.".into());
                }
                let name = project_name(start);
                (format!("Start “{name}”"), Some(excerpt(&start.prompt)))
            }
            Request::Message { project, message } => {
                let state = self.named(Some(project), cx)?;
                let briefing = self.clients.briefing.contains(&state.entity_id());
                if briefing || state.read(cx).agent_busy() {
                    return Err(format!(
                        "The agent of “{}” is still working; wait_for_projects first.",
                        name(&state)
                    ));
                }
                (
                    format!("Message “{}”", name(&state)),
                    Some(excerpt(message)),
                )
            }
            Request::Save { project } => {
                let state = self.named(project.as_deref(), cx)?;
                if state.read(cx).file().is_none() {
                    return Err(format!(
                        "“{}” has no file yet: the user saves it in Flikk first.",
                        name(&state)
                    ));
                }
                (format!("Save “{}”", name(&state)), None)
            }
            Request::Close { project } => {
                let state = self.named(Some(project), cx)?;
                if state.read(cx).is_unsettled() {
                    return Err(format!(
                        "“{}” has unsaved changes; save_project first.",
                        name(&state)
                    ));
                }
                (format!("Close “{}”", name(&state)), None)
            }
        })
    }

    fn carry_out(
        &mut self,
        request: Request,
        reply: AppReply,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The user may have answered much later: it is looked at again.
        if let Err(why) = self.describe(&request, cx) {
            return reply.failed(why);
        }
        let done = match request {
            Request::List => Ok(self.told_all(&self.shown(cx), cx)),
            Request::Start(start) => self.start_project(start, window, cx),
            Request::Message { project, message } => self.named(Some(&project), cx).map(|state| {
                state.update(cx, |s, cx| s.send_chat(message, cx));
                self.told(&state, cx)
            }),
            Request::Wait { projects, timeout } => {
                let named: Result<Vec<_>, String> = if projects.is_empty() {
                    Ok(self.shown(cx))
                } else {
                    let named = projects.iter();
                    named.map(|project| self.named(Some(project), cx)).collect()
                };
                match named {
                    Ok(projects) => {
                        self.clients.waits.push(Wait {
                            projects,
                            until: Instant::now() + timeout,
                            reply,
                        });
                        return;
                    }
                    Err(why) => Err(why),
                }
            }
            Request::Save { project } => self.named(project.as_deref(), cx).and_then(|state| {
                let file = state.read(cx).file().map(PathBuf::from);
                let saved =
                    file.is_some_and(|file| state.update(cx, |s, cx| s.save_project(file, cx)));
                if saved {
                    Ok(self.told(&state, cx))
                } else {
                    Err(why_not(&state, "It could not be saved", cx))
                }
            }),
            Request::Close { project } => self.named(Some(&project), cx).map(|state| {
                let closed = json!({ "closed": state.read(cx).bus.project().name });
                self.close(&state, window, cx);
                closed
            }),
        };
        match done {
            Ok(result) => reply.done(&result),
            Err(why) => reply.failed(why),
        }
    }

    /// Make the project `start` asks for, saved in its folder, and send
    /// its agent the prompt once its footage is in. The project in the
    /// editor stays there.
    fn start_project(
        &mut self,
        start: Start,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        let dir = match &start.folder {
            Some(folder) => folder.clone(),
            None => crate::ui::project_settings::new_project_dir(&self.editor, cx),
        };
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Could not make {}: {e}", dir.display()))?;
        let name = crate::ui::project_settings::free_name(&dir, &project_name(&start));
        let path = dir.join(format!("{}.{EXTENSION}", sanitize(&name)));
        let mut settings = SequenceSettings::default();
        if let Some((width, height)) = start.format {
            (settings.width, settings.height) = (width, height);
        }
        if let Some(rate) = start.rate {
            settings.rate = rate;
        }
        let state = self.add(persistence::new_project(&name, settings), window, cx);
        if !state.update(cx, |s, cx| s.save_project(path, cx)) {
            let why = why_not(&state, "The project could not be saved", cx);
            self.drop_session(&state, cx);
            return Err(why);
        }
        self.brief(&state, start.prompt, start.footage, start.style, cx);
        cx.notify();
        Ok(self.told(&state, cx))
    }

    /// The open project `named` by its id, its name or its file; the one
    /// in the editor without a name.
    fn named(&self, named: Option<&str>, cx: &App) -> Result<Entity<EditorState>, String> {
        let Some(named) = named else {
            return Ok(self.editor.clone());
        };
        let wanted = named.trim().to_lowercase();
        let matches = |state: &&Entity<EditorState>| {
            let s = state.read(cx);
            let project = s.bus.project();
            let id = project.id.0.simple().to_string();
            (wanted.len() >= 8 && id.starts_with(&wanted.replace('-', "")))
                || project.name.to_lowercase() == wanted
                || s.file()
                    .is_some_and(|f| f.to_string_lossy() == named.trim())
        };
        let mut found = self.states().filter(matches);
        match (found.next(), found.next()) {
            (Some(state), None) => Ok(state.clone()),
            (Some(_), Some(_)) => Err(format!(
                "Several open projects are called `{named}`; name it by its id."
            )),
            (None, _) => Err(format!(
                "No open project is `{named}`; list_projects shows the open ones."
            )),
        }
    }

    /// The projects a client is shown: those on the home page, and the
    /// one in the editor.
    fn shown(&self, cx: &App) -> Vec<Entity<EditorState>> {
        let shown = self.states().filter(|state| {
            let s = state.read(cx);
            s.is_listed() || s.in_editor()
        });
        shown.cloned().collect()
    }

    /// Whether an agent or a render of `state`'s project is at work.
    fn at_work(&self, state: &Entity<EditorState>, cx: &App) -> bool {
        let s = state.read(cx);
        let rendering = s.chat_renders().iter().any(|job| !job.state.is_finished());
        rendering
            || s.agent_status() == AgentStatus::Working
            || self.clients.briefing.contains(&state.entity_id())
    }

    /// Answer the waits whose projects are all at rest (a closed one is),
    /// and those out of time.
    fn end_waits(&mut self, cx: &mut Context<Self>) {
        if self.clients.waits.is_empty() {
            return;
        }
        let now = Instant::now();
        let (over, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.clients.waits)
            .into_iter()
            .partition(|wait| {
                let working = |state: &Entity<EditorState>| {
                    self.session(state).is_some() && self.at_work(state, cx)
                };
                now >= wait.until || !wait.projects.iter().any(working)
            });
        self.clients.waits = waiting;
        for wait in over {
            let open: Vec<_> = (wait.projects.into_iter())
                .filter(|state| self.session(state).is_some())
                .collect();
            wait.reply.done(&self.told_all(&open, cx));
        }
    }

    fn told_all(&self, states: &[Entity<EditorState>], cx: &App) -> Value {
        let told: Vec<Value> = states.iter().map(|state| self.told(state, cx)).collect();
        json!({ "projects": told })
    }

    /// Where `state`'s project stands, as a client is told.
    fn told(&self, state: &Entity<EditorState>, cx: &App) -> Value {
        let s = state.read(cx);
        let project = s.bus.project();
        let sequence = &project.timeline.settings;
        let rate = sequence.rate;
        let fps = (flikk_core::Rate::SEQUENCE.iter())
            .find(|(offered, _)| *offered == rate)
            .map_or_else(
                || format!("{:.3}", rate.fps()),
                |(_, name)| (*name).to_owned(),
            );
        let briefing = self.clients.briefing.contains(&state.entity_id());
        let agent = match s.agent_status() {
            _ if briefing => "working",
            AgentStatus::Idle => "idle",
            AgentStatus::Working => "working",
            AgentStatus::Waiting => "waiting on the user",
            AgentStatus::Failed => "failed",
        };
        let mut told = json!({
            "project": project.id.to_string(),
            "name": project.name,
            "file": s.file().map(|f| f.display().to_string()),
            "unsaved_changes": s.is_dirty(),
            "in_editor": s.in_editor(),
            "format": format!("{}x{} at {fps} fps", sequence.width, sequence.height),
            "duration": project.timeline.duration().as_secs_f64(),
            "agent": agent,
        });
        let chat = s.chat();
        let said = (chat.iter().rev())
            .take_while(|m| m.role == ChatRole::Agent)
            .find(|m| !m.text.trim().is_empty());
        if let Some(said) = said {
            told["agent_said"] = json!(tail(said.text.trim(), SAID_MAX));
        }
        if let Some(question) = s.open_question().and_then(|ix| chat.get(ix)) {
            told["agent_asks"] = json!({ "question": question.text, "choices": question.choices });
        }
        if let Some(error) = s.agent_error() {
            told["agent_error"] = json!(error);
        }
        let to_review: usize = (s.bus.change_sets().iter())
            .filter(|set| !set.author.is_user() && set.state != ChangeSetState::Resolved)
            .map(|set| set.pending_items().count())
            .sum();
        if to_review > 0 {
            told["changes_waiting_for_review"] = json!(to_review);
        }
        let waiting: Vec<String> = (s.pending_approvals().into_iter())
            .filter(|p| !p.request.allowed)
            .map(|p| p.request.summary)
            .collect();
        if !waiting.is_empty() {
            told["waiting_for_approval"] = json!(waiting);
        }
        let renders: Vec<Value> = (s.chat_renders().iter())
            .map(|job| {
                let mut render = json!({ "file": job.output.display().to_string() });
                match &job.state {
                    RenderState::Queued => render["state"] = json!("queued"),
                    RenderState::Running { fraction } => {
                        render["state"] = json!("rendering");
                        render["done"] = json!((fraction * 100.).round() / 100.);
                    }
                    RenderState::Done => render["state"] = json!("done"),
                    RenderState::Failed { message } => {
                        render["state"] = json!("failed");
                        render["error"] = json!(message);
                    }
                    RenderState::Canceled => render["state"] = json!("canceled"),
                }
                render
            })
            .collect();
        if !renders.is_empty() {
            told["renders"] = json!(renders);
        }
        told
    }
}

/// The name of the project `start` asks for, before it is made free in
/// its folder.
fn project_name(start: &Start) -> String {
    match &start.name {
        Some(name) => sanitize(name),
        None => crate::ui::welcome::name_from(&start.prompt, &start.footage),
    }
}

/// What the status line of `state` says went wrong, else `otherwise`.
fn why_not(state: &Entity<EditorState>, otherwise: &str, cx: &App) -> String {
    let status = state.read(cx).status.clone();
    status.unwrap_or_else(|| format!("{otherwise}."))
}

/// The first line of `text`, cut to what the question shows.
fn excerpt(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    match line.char_indices().nth(DETAIL_MAX) {
        Some((cut, _)) => format!("{}…", line[..cut].trim_end()),
        None if line.len() < text.trim().len() => format!("{line}…"),
        None => line.to_owned(),
    }
}

/// The last `max` characters of `text`.
fn tail(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_owned();
    }
    let kept: String = text.chars().skip(count - max).collect();
    format!("…{kept}")
}
