//! The agent host as the editor drives it: queue a turn with [`send`], call
//! [`pump`] on the UI thread every frame-ish, read back `AgentEvent`s.
//!
//! One turn = one backend subprocess. Edits the agent makes during the turn
//! land in its open change set (the bus opens it on the first proposal); at
//! the end of the turn the host emits `ChangeSetReady` if it has items, then
//! `TurnDone`.
//!
//! External MCP clients (Claude Code, Codex… running `flikk mcp`
//! against the published `bridge.json`) are served between and during
//! turns. They act as the agent with the same permissions,
//! under their own author, so their proposals collect in a separate
//! "External agent" change set. That set is offered for review when the
//! client disconnects or after [`EXTERNAL_IDLE`] without a call (a client
//! usually stays connected for its whole session, so the pause between
//! bursts of work is what ends one); later calls open a new set.
//!
//! [`send`]: AgentHost::send
//! [`pump`]: AgentHost::pump

use crate::backend::{self, BackendEvent, McpLaunch, RunningTurn, TurnSpec};
use crate::bridge::{self, BridgeServer, Incoming, IncomingKind, Origin, Reply};
use crate::tools::groups::{self, Usage};
use crate::tools::{self, ApprovalRequest, Done, JobDone, Memory, Step, Stop, ToolCtx};
use crate::{prompt, session};
use flikk_core::agent::{ActivityEntry, AgentEvent, AgentSettings, Backend, EditorContext};
use flikk_core::bus::{Author, CommandBus};
use flikk_core::changeset::ChangeSetState;
use flikk_core::command::ToolCategory;
use flikk_core::jobs::JobQueue;
use flikk_core::memory::{SourceFact, StyleRule};
use flikk_core::model::MediaItem;
use flikk_core::services::Services;
use flikk_core::skills::{self, Kind};
use flikk_core::{ChangeSetId, ProjectId};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

/// Quiet time after which an external client's change set goes to review.
pub(crate) const EXTERNAL_IDLE: Duration = Duration::from_secs(90);

/// The author external clients' proposals are made under.
pub(crate) fn external_author() -> Author {
    Author::External {
        name: "External agent".into(),
    }
}

/// Calls' jobs running at once; the others wait their turn, the order
/// called. Enough for a turn's calls made together (frames, a measure, a
/// search) to run side by side, few enough that they don't swamp the
/// machine; their GPU work takes turns besides.
const JOBS_AT_ONCE: usize = 4;

/// What a job stopped with its turn answers: not an error.
fn stopped() -> Done {
    Done::text("Stopped", "Stopped.")
}

/// External clients connected now and when one last called.
#[derive(Default)]
struct External {
    connections: HashSet<u64>,
    last_call: Option<Instant>,
    jobs: usize,
    /// Editor context and settings of the latest turn, used for their calls.
    context: EditorContext,
    settings: AgentSettings,
    /// Never stopped: a client stops its own calls.
    stop: Stop,
}

/// A memory, deliver or generate request waiting for the user. Approving
/// it is the UI's job (through the store, deliver crate or a provider); the
/// call that asked waits for it, see [`AgentHost::report_outcome`].
#[derive(Clone, Debug, PartialEq)]
pub struct PendingApproval {
    pub id: u64,
    pub request: ApprovalRequest,
    /// Unix seconds.
    pub at: i64,
    /// The project it was asked on.
    pub project: ProjectId,
    /// The chat whose turn asked; none for an external client.
    pub chat: Option<String>,
}

/// What became of an approval whose call no longer waits, for the next
/// turn of the chat that asked.
struct Outcome {
    project: ProjectId,
    chat: String,
    text: String,
}

/// Where the agent is with the open chat's messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnStatus {
    /// Nothing queued or under way: a message is taken.
    Idle,
    /// A message waits for the next pump to start its turn.
    Queued,
    /// A turn is under way.
    Running,
    /// The turn was cancelled; it ends on the next pump.
    Stopping,
}

impl TurnStatus {
    /// A message is queued or its turn under way, so another is refused.
    pub fn is_busy(self) -> bool {
        self != Self::Idle
    }

    /// A turn has started and not ended. The agent's open change set is
    /// that turn's, and goes to review when it ends.
    pub fn in_turn(self) -> bool {
        matches!(self, Self::Running | Self::Stopping)
    }
}

/// What the agent says when the backend lost a session it can't re-run.
const SESSION_LOST: &str = "The agent's session was lost; send your message again";

struct Queued {
    message: String,
    context: EditorContext,
    settings: AgentSettings,
}

struct Turn {
    /// Numbers turns, so a job of an earlier one is told apart.
    id: u64,
    /// Stops what its jobs are doing when it is cancelled.
    stop: Stop,
    author: Author,
    backend: Backend,
    project: flikk_core::ProjectId,
    /// Chat session the turn belongs to; its backend session is resumed.
    chat: String,
    /// The chat's set the turn reopened, with how many items it had then.
    carried: Option<(ChangeSetId, usize)>,
    context: EditorContext,
    settings: AgentSettings,
    /// Kept to re-run once without `--resume` if the session is gone.
    spec: TurnSpec,
    running: Option<RunningTurn>,
    /// Off-thread tool jobs still running.
    jobs: usize,
    phase: Phase,
    /// Something came of the turn that running it again would repeat: text
    /// shown, a tool call made (with the proposals, jobs and approvals it
    /// led to) or one of the backend's own tools used.
    acted: bool,
    /// Tool calls the backend began and has no result for (see
    /// [`BackendEvent::OpenCalls`]).
    open_calls: usize,
    /// Tool groups the chat lists, fixed when it started (see
    /// `tools::groups`).
    groups: BTreeSet<ToolCategory>,
    usage: Usage,
    /// The backend's own tool calls still running: their key and call id.
    web_calls: HashMap<String, u64>,
}

/// Where a started turn is.
enum Phase {
    /// The backend is working on it.
    Running,
    /// The backend ended it, with an error if it failed. The turn ends
    /// once its jobs have.
    Finished(Option<String>),
    /// Cancelled: it ends on the next pump.
    Stopping,
}

impl Turn {
    fn stopping(&self) -> bool {
        matches!(self.phase, Phase::Stopping)
    }
}

/// A finished job: who started it, in which turn and on which project, and
/// the call id its start was shown under.
struct JobResult {
    origin: Origin,
    turn: Option<u64>,
    project: ProjectId,
    reply: Reply,
    tool: String,
    args: Value,
    out: JobDone,
    call: u64,
}

pub struct AgentHost {
    bridge: Option<BridgeServer>,
    /// Chat session whose backend conversation the next turn continues.
    chat: String,
    memory: Memory,
    queued: Option<Queued>,
    turn: Option<Turn>,
    events: Vec<AgentEvent>,
    /// Where the calls' jobs run, [`JOBS_AT_ONCE`] at a time.
    jobs: JobQueue<u64>,
    jobs_tx: Sender<JobResult>,
    jobs_rx: Receiver<JobResult>,
    approvals: Vec<PendingApproval>,
    next_approval: u64,
    /// Replies of the calls waiting on an approval, by its id: the call
    /// stays open until the user decides, so the turn carries on with what
    /// came of it.
    held: HashMap<u64, (Origin, Reply)>,
    /// Where each approval still to be reported was asked: its project, and
    /// its chat (none for an external client).
    asked: HashMap<u64, (ProjectId, Option<String>)>,
    /// What became of approval requests whose call was no longer waiting
    /// (the turn ended), told to the agent on that chat's next turn.
    outcomes: Vec<Outcome>,
    external: External,
    /// Id of the next call shown while it runs.
    next_call: u64,
    next_turn: u64,
    /// What jobs made for a project that was no longer open when they
    /// finished, for the editor to keep for it.
    orphans: Vec<(ProjectId, MediaItem)>,
}

impl Default for AgentHost {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentHost {
    pub fn new() -> Self {
        let (jobs_tx, jobs_rx) = mpsc::channel();
        Self {
            bridge: None,
            chat: session::MAIN_CHAT.to_owned(),
            memory: Memory::default(),
            queued: None,
            turn: None,
            events: Vec::new(),
            jobs: JobQueue::new("flikk-agent-job", JOBS_AT_ONCE),
            jobs_tx,
            jobs_rx,
            approvals: Vec::new(),
            next_approval: 1,
            held: HashMap::new(),
            asked: HashMap::new(),
            outcomes: Vec::new(),
            external: External::default(),
            next_call: 1,
            next_turn: 1,
            orphans: Vec::new(),
        }
    }

    /// Start the bridge now (rather than with the first turn) and publish
    /// it in `bridge.json`, so external MCP clients can reach the editor.
    pub fn start_bridge(&mut self) -> std::io::Result<PathBuf> {
        if self.bridge.is_none() {
            self.bridge = Some(BridgeServer::start()?);
        }
        self.bridge.as_mut().unwrap().publish()
    }

    /// Withdraw `bridge.json` (on exit; best-effort).
    pub fn stop_bridge(&mut self) {
        if let Some(b) = self.bridge.as_mut() {
            b.unpublish();
        }
    }

    /// Whether a message is queued, its turn under way or being stopped.
    pub fn status(&self) -> TurnStatus {
        match (&self.turn, &self.queued) {
            (Some(turn), _) if turn.stopping() => TurnStatus::Stopping,
            (Some(_), _) => TurnStatus::Running,
            (None, Some(_)) => TurnStatus::Queued,
            (None, None) => TurnStatus::Idle,
        }
    }

    /// Continue chat session `chat` from the next turn on.
    pub fn set_chat(&mut self, chat: impl Into<String>) {
        self.chat = chat.into();
    }

    /// Style rules and source facts; rules apply by scope, facts are fetched
    /// by range inside tools.
    pub fn set_memory(&mut self, rules: Vec<StyleRule>, facts: Vec<SourceFact>) {
        self.memory.rules = rules;
        self.memory.facts = facts;
    }

    /// The selected style profile's caption styles, as saved.
    pub fn set_caption_styles(&mut self, styles: Vec<flikk_core::memory::CaptionStyle>) {
        self.memory.caption_styles = styles;
    }

    /// Queue a turn; it starts on the next [`pump`](Self::pump).
    pub fn send(&mut self, message: String, context: EditorContext, settings: &AgentSettings) {
        if self.status().is_busy() {
            self.events.push(AgentEvent::Error {
                message: "The agent is still working on the last message".into(),
            });
            return;
        }
        // Attachments are for this chat's own turns, never other clients.
        self.external.context = EditorContext {
            attachments: Vec::new(),
            ..context.clone()
        };
        self.external.settings = settings.clone();
        self.queued = Some(Queued {
            message,
            context,
            settings: settings.clone(),
        });
    }

    /// Stop the turn: kill the subprocess and stop the generations and
    /// waits its jobs started. The partial change set is still offered for
    /// review on the next pump.
    pub fn cancel(&mut self) {
        self.queued = None;
        if let Some(turn) = self.turn.as_mut() {
            turn.phase = Phase::Stopping;
            turn.stop.stop();
            if let Some(r) = &turn.running {
                r.kill();
            }
        }
    }

    /// What jobs made for projects that were closed before they finished,
    /// with the project each belongs to.
    pub fn take_orphans(&mut self) -> Vec<(ProjectId, MediaItem)> {
        std::mem::take(&mut self.orphans)
    }

    /// The requests waiting for the user in the open chat, and those of
    /// external clients.
    pub fn pending_approvals(&self) -> Vec<PendingApproval> {
        self.approvals
            .iter()
            .filter(|a| a.chat.as_deref().is_none_or(|c| c == self.chat))
            .cloned()
            .collect()
    }

    /// The user opened another project: withdraw the requests `project`'s
    /// turns left waiting. Its chats' agents are told on their next turn.
    pub fn close_project(&mut self, project: ProjectId) {
        let (gone, kept) = std::mem::take(&mut self.approvals)
            .into_iter()
            .partition(|a| a.project == project);
        self.approvals = kept;
        for approval in gone {
            let PendingApproval { id, request, .. } = approval;
            self.report_outcome(
                id,
                format!(
                    "withdrawn: {} (the user opened another project before deciding)",
                    request.summary
                ),
                true,
            );
        }
    }

    /// Remove a request once the user approved or denied it.
    pub fn resolve_approval(&mut self, id: u64) -> Option<PendingApproval> {
        let ix = self.approvals.iter().position(|a| a.id == id)?;
        Some(self.approvals.remove(ix))
    }

    /// Demo only: `request` waiting on the user for `project`, as a turn's
    /// call leaves one, with no call waiting on its outcome.
    pub fn seed_approval(&mut self, request: ApprovalRequest, project: ProjectId) {
        let id = self.next_approval;
        self.next_approval += 1;
        self.approvals.push(PendingApproval {
            id,
            request,
            at: now(),
            project,
            chat: None,
        });
    }

    /// The user changed waiting request `id` before deciding: it now asks
    /// for `args`, shown as `summary`. Nothing when it is no longer waiting.
    pub fn revise_approval(&mut self, id: u64, args: Value, summary: String) {
        if let Some(approval) = self.approvals.iter_mut().find(|a| a.id == id) {
            approval.request.args = args;
            approval.request.summary = summary;
        }
    }

    /// Tell the agent what became of approval request `id` (`"approved:
    /// Style rule: …"`): as the result of the call that asked, which is
    /// still waiting, else at the start of its next turn.
    pub fn report_outcome(&mut self, id: u64, outcome: String, failed: bool) {
        let asked = self.asked.remove(&id);
        match self.held.remove(&id) {
            Some((origin, reply)) => {
                // An external client's quiet time starts from the answer.
                if let Origin::External(_) = origin {
                    self.external.last_call = Some(Instant::now());
                }
                reply.result(if failed {
                    bridge::ToolResult::error(outcome)
                } else {
                    bridge::ToolResult::text(outcome)
                });
            }
            // An external client's call that gave up waiting has nobody to
            // tell; the editor's own agent never asked.
            None => {
                if let Some((project, Some(chat))) = asked {
                    self.outcomes.push(Outcome {
                        project,
                        chat,
                        text: outcome,
                    });
                }
            }
        }
    }

    /// Stop holding the calls from `origin`: nobody waits for them any more,
    /// so their outcomes go to the next turn.
    fn release_held(&mut self, origin: Origin) {
        self.held.retain(|_, (o, _)| *o != origin);
    }

    pub fn pump(&mut self, bus: &mut CommandBus, services: &Services) -> Vec<AgentEvent> {
        if self.turn.is_none()
            && let Some(q) = self.queued.take()
        {
            self.start(q, bus, services);
        }
        self.serve_calls(bus, services);
        self.drain_jobs(bus, services);
        self.drain_backend();
        self.maybe_finish(bus);
        self.maybe_close_external(bus);
        std::mem::take(&mut self.events)
    }

    // ------------------------------------------------------------- turns

    fn start(&mut self, q: Queued, bus: &mut CommandBus, services: &Services) {
        let author = Author::Agent;
        // A set left open by an earlier turn that never finished.
        close_open_sets(bus, &author, None, &mut self.events);
        bus.set_chat(&self.chat);
        // The chat's proposals still waiting for review: this turn sees them
        // and adds to them.
        let carried = bus.reopen_change_set(&author);
        let project = bus.view_for(&author);
        let project_id = project.id;
        let (skill, text) = match skills::invocation(&q.message) {
            Some(run) => (Some(run.name), run.prompt),
            None => (None, q.message.clone()),
        };
        let style = q
            .settings
            .style
            .as_deref()
            .and_then(|id| skills::get(Kind::Style, id));
        let mut message = prompt::turn_message(
            &text,
            project,
            &q.context,
            &self.memory.rules,
            prompt::Asked {
                skill: skill.as_deref(),
                style: style.as_ref(),
            },
        );
        // What came of this chat's own requests, and no other's.
        let (ours, others): (Vec<_>, Vec<_>) = std::mem::take(&mut self.outcomes)
            .into_iter()
            .partition(|o| o.project == project_id && o.chat == self.chat);
        self.outcomes = others;
        if !ours.is_empty() {
            let outcomes: Vec<_> = ours.into_iter().map(|o| o.text).collect();
            let outcomes = outcomes.join("\n");
            message = format!("<approvals>\n{outcomes}\n</approvals>\n\n{message}");
        }
        let images = tools::image::offered(&q.settings, services);
        let system_prompt = prompt::system_prompt(&q.settings, images);
        let backend = q.settings.backend;
        let resume = session::load(project.id, &self.chat, backend);
        // A new session doesn't have the chat's earlier messages.
        if resume.is_none() {
            message = prompt::with_recap(message, &q.context.history);
        }
        let picked = groups::pick(&text, project, &q.context);
        // A conversation keeps the list it started with, so its cached
        // prompt stays valid; a new one starts from this message's picks.
        let key = session::key(project.id, &self.chat, backend);
        let groups = resume
            .as_ref()
            .and_then(|_| groups::stored(&key))
            .unwrap_or_else(|| picked.clone());
        groups::store(&key, &groups);

        let launch = (|| -> std::io::Result<(McpLaunch, PathBuf, PathBuf)> {
            if self.bridge.is_none() {
                self.bridge = Some(BridgeServer::start()?);
            }
            let bridge = self.bridge.as_ref().unwrap();
            let (command, args) = backend::mcp_command()?;
            let mcp = McpLaunch {
                command,
                args,
                env: vec![
                    (bridge::ENV_ADDR.into(), bridge.addr().to_string()),
                    (bridge::ENV_TOKEN.into(), bridge.token().into()),
                ],
            };
            let dir = session::agent_dir();
            let workdir = dir.join("work");
            let run_dir = dir
                .join("run")
                .join(uuid::Uuid::new_v4().simple().to_string());
            std::fs::create_dir_all(&workdir)?;
            std::fs::create_dir_all(&run_dir)?;
            Ok((mcp, workdir, run_dir))
        })();
        let (mcp, workdir, run_dir) = match launch {
            Ok(l) => l,
            Err(e) => {
                self.fail_now(
                    bus,
                    &author,
                    carried,
                    format!("Could not start the agent: {e}"),
                );
                return;
            }
        };

        let spec = TurnSpec {
            system_prompt,
            message,
            resume,
            model: q.settings.models.get(backend).clone(),
            mcp: Some(mcp),
            web: q.settings.web,
            workdir,
            run_dir,
        };
        let running = match backend::start(backend::adapter(backend), spec.clone()) {
            Ok(r) => r,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&spec.run_dir);
                let message = format!("Could not start {}: {e}", backend.label());
                self.fail_now(bus, &author, carried, message);
                return;
            }
        };
        self.events.push(AgentEvent::Progress {
            label: skill.map_or_else(|| "Thinking".to_owned(), |r| format!("Running /{r}")),
            fraction: None,
        });
        let id = self.next_turn;
        self.next_turn += 1;
        self.turn = Some(Turn {
            id,
            stop: Stop::default(),
            author,
            backend,
            project: project_id,
            chat: self.chat.clone(),
            carried,
            context: q.context,
            settings: q.settings,
            spec,
            running: Some(running),
            jobs: 0,
            phase: Phase::Running,
            acted: false,
            open_calls: 0,
            usage: Usage::new(picked, groups.clone()),
            groups,
            web_calls: HashMap::new(),
        });
    }

    fn fail_now(
        &mut self,
        bus: &mut CommandBus,
        author: &Author,
        carried: Option<(ChangeSetId, usize)>,
        message: String,
    ) {
        close_open_sets(bus, author, carried, &mut self.events);
        self.events.push(AgentEvent::Error { message });
        self.events.push(AgentEvent::TurnDone);
    }

    fn drain_backend(&mut self) {
        let Some(turn) = self.turn.as_mut() else {
            return;
        };
        let Some(running) = &turn.running else {
            return;
        };
        let stopping = turn.stopping();
        for event in running.events.try_iter() {
            match event {
                BackendEvent::Session(id) => {
                    session::store(turn.project, &turn.chat, turn.backend, Some(&id));
                }
                BackendEvent::TextDelta(text) => {
                    turn.acted = true;
                    if !stopping {
                        self.events.push(AgentEvent::TextDelta { text });
                    }
                }
                BackendEvent::ToolStarted {
                    key,
                    tool,
                    args,
                    summary,
                } => {
                    turn.acted = true;
                    if !stopping {
                        let call = self.next_call;
                        self.next_call += 1;
                        turn.web_calls.insert(key, call);
                        self.events.push(AgentEvent::ToolStarted {
                            call,
                            entry: started(tool.into(), args, summary, call),
                        });
                    }
                }
                BackendEvent::Tool {
                    key,
                    tool,
                    args,
                    summary,
                    ok,
                } => {
                    turn.acted = true;
                    if !stopping {
                        let entry = ActivityEntry {
                            at: now(),
                            tool: tool.into(),
                            args,
                            summary,
                            images: Vec::new(),
                            excerpts: Vec::new(),
                            ok,
                            running: None,
                        };
                        let call = key.and_then(|k| turn.web_calls.remove(&k));
                        self.events.push(match call {
                            Some(call) => AgentEvent::ToolFinished { call, entry },
                            None => AgentEvent::Activity { entry },
                        });
                    }
                }
                BackendEvent::OpenCalls(n) => turn.open_calls = n,
                BackendEvent::Finished { error } => {
                    if !stopping {
                        turn.phase = Phase::Finished(error);
                    }
                    break;
                }
            }
        }
        // The backend forgot the session: forget it too. A turn nothing came
        // of yet runs again fresh; one that already acted would repeat what
        // it did, so it ends and the user sends the message again.
        let lost = matches!(&turn.phase, Phase::Finished(Some(e)) if is_lost_session(e));
        if !lost {
            return;
        }
        session::store(turn.project, &turn.chat, turn.backend, None);
        if turn.acted {
            turn.phase = Phase::Finished(Some(SESSION_LOST.into()));
        } else if turn.spec.resume.is_some() {
            let mut spec = turn.spec.clone();
            spec.resume = None;
            spec.message = prompt::with_recap(spec.message, &turn.context.history);
            match backend::start(backend::adapter(turn.backend), spec.clone()) {
                Ok(r) => {
                    turn.spec = spec;
                    turn.running = Some(r);
                    turn.phase = Phase::Running;
                    turn.open_calls = 0;
                }
                Err(e) => turn.phase = Phase::Finished(Some(e.to_string())),
            }
        }
    }

    fn maybe_finish(&mut self, bus: &mut CommandBus) {
        let Some(turn) = self.turn.as_ref() else {
            return;
        };
        let done = match turn.phase {
            Phase::Running => false,
            Phase::Finished(_) => turn.jobs == 0,
            Phase::Stopping => true,
        };
        if !done {
            return;
        }
        let turn = self.turn.take().unwrap();
        // Killed (or crashed) with a tool call open, the backend's session
        // holds a call without its result, which a resume can't continue
        // from: the chat's next turn starts a new session. What the host
        // still runs or holds for the turn covers calls the backend's
        // last events, cut off by the kill, did not report.
        let stopping = turn.stopping();
        let held = self.held.values().any(|(o, _)| *o == Origin::Turn);
        let pending = turn.jobs > 0 || held || !turn.web_calls.is_empty();
        if turn.open_calls > 0 || (stopping && pending) {
            session::store(turn.project, &turn.chat, turn.backend, None);
        }
        self.release_held(Origin::Turn);
        match &turn.phase {
            Phase::Stopping => {
                if let Some(r) = &turn.running {
                    r.kill();
                }
            }
            Phase::Finished(Some(error)) => self.events.push(AgentEvent::Error {
                message: error.clone(),
            }),
            Phase::Finished(None) | Phase::Running => {}
        }
        close_open_sets(bus, &turn.author, turn.carried, &mut self.events);
        turn.usage.log(&turn.settings, &turn.context, now());
        let _ = std::fs::remove_dir_all(&turn.spec.run_dir);
        self.events.push(AgentEvent::TurnDone);
    }

    // ------------------------------------------------------------- tool calls

    fn serve_calls(&mut self, bus: &mut CommandBus, services: &Services) {
        let Some(bridge) = self.bridge.as_ref() else {
            return;
        };
        for Incoming {
            origin,
            request,
            reply,
        } in bridge.drain()
        {
            match (origin, request) {
                (_, IncomingKind::Closed) => {
                    if let Origin::External(id) = origin {
                        self.external.connections.remove(&id);
                        self.release_held(origin);
                    }
                }
                (Origin::Turn, IncomingKind::List) => match self.turn.as_mut() {
                    Some(turn) => {
                        let list = tools::list(bus, Some(&turn.groups), &turn.settings, services);
                        turn.usage.listed = list.len();
                        turn.usage.listed_bytes = list.iter().map(|t| t.to_string().len()).sum();
                        reply.tools(list);
                    }
                    None => reply.tools(tools::list(bus, None, &self.external.settings, services)),
                },
                (Origin::External(id), IncomingKind::List) => {
                    self.external.connections.insert(id);
                    reply.tools(tools::list(bus, None, &self.external.settings, services));
                }
                (Origin::Turn, IncomingKind::Call { tool, args }) => {
                    let Some(turn) = self.turn.as_mut().filter(|t| !t.stopping()) else {
                        reply.result(bridge::ToolResult::error("No turn is running."));
                        continue;
                    };
                    turn.acted = true;
                    let via_run = tool == groups::RUN_TOOL;
                    let (tool, args) = groups::unwrap_run(tool, args);
                    turn.usage.calls.push((tool.clone(), via_run));
                    if tool == groups::LOAD_TOOLS {
                        let done = groups::load(
                            bus,
                            &turn.groups,
                            &mut turn.usage.loaded,
                            tools::image::offered(&turn.settings, services),
                            &args,
                        );
                        let project = bus.project().id;
                        self.finish_call(origin, project, reply, tool, args, done, None);
                        continue;
                    }
                    let mut ctx = ToolCtx {
                        bus,
                        services,
                        author: turn.author.clone(),
                        external: false,
                        editor: &turn.context,
                        settings: &turn.settings,
                        memory: &self.memory,
                        stop: &turn.stop,
                    };
                    match tools::serve(&mut ctx, &tool, &args) {
                        Step::Done(done) => {
                            let project = bus.project().id;
                            self.finish_call(origin, project, reply, tool, args, *done, None)
                        }
                        Step::Job(job) => {
                            turn.jobs += 1;
                            let started = (Some(turn.id), bus.project().id);
                            self.spawn_job(origin, started, reply, tool, args, job);
                        }
                    }
                }
                (Origin::External(id), IncomingKind::Call { tool, args }) => {
                    let (tool, args) = groups::unwrap_run(tool, args);
                    self.external.connections.insert(id);
                    self.external.last_call = Some(Instant::now());
                    let mut ctx = ToolCtx {
                        bus,
                        services,
                        author: external_author(),
                        external: true,
                        editor: &self.external.context,
                        settings: &self.external.settings,
                        memory: &self.memory,
                        stop: &self.external.stop,
                    };
                    match tools::serve(&mut ctx, &tool, &args) {
                        Step::Done(done) => {
                            let project = bus.project().id;
                            self.finish_call(origin, project, reply, tool, args, *done, None)
                        }
                        Step::Job(job) => {
                            self.external.jobs += 1;
                            let started = (None, bus.project().id);
                            self.spawn_job(origin, started, reply, tool, args, job);
                        }
                    }
                }
            }
        }
    }

    /// Run `job` off-thread, shown in the chat while it runs: queued
    /// behind the others when [`JOBS_AT_ONCE`] run already. `from` is the
    /// turn (for the editor's own calls) and project it started in.
    fn spawn_job(
        &mut self,
        origin: Origin,
        from: (Option<u64>, ProjectId),
        reply: Reply,
        tool: String,
        args: Value,
        job: tools::Job,
    ) {
        let call = self.next_call;
        self.next_call += 1;
        self.events.push(AgentEvent::ToolStarted {
            call,
            entry: started(tool.clone(), args.clone(), String::new(), call),
        });
        let tx = self.jobs_tx.clone();
        let (turn, project) = from;
        self.jobs.clear_finished();
        self.jobs.push_first(call, tool.clone(), None, move |_| {
            // A job that panics still answers its call and counts as done,
            // so its turn or the external set doesn't wait on it forever.
            let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job))
                .unwrap_or_else(|_| JobDone::Done(Done::error("The tool failed unexpectedly.")));
            let _ = tx.send(JobResult {
                origin,
                turn,
                project,
                reply,
                tool,
                args,
                out,
                call,
            });
            Ok(())
        });
    }

    fn drain_jobs(&mut self, bus: &mut CommandBus, services: &Services) {
        let results: Vec<_> = self.jobs_rx.try_iter().collect();
        for JobResult {
            origin,
            turn,
            project,
            reply,
            tool,
            args,
            out,
            call,
        } in results
        {
            // A job of an earlier turn (cancelled, or ended) is not this
            // one's: it neither counts here nor acts for it.
            let live = match origin {
                Origin::Turn => {
                    let live = self.turn.as_ref().is_some_and(|t| Some(t.id) == turn);
                    if live && let Some(t) = self.turn.as_mut() {
                        t.jobs = t.jobs.saturating_sub(1);
                    }
                    live
                }
                Origin::External(_) => {
                    self.external.jobs = self.external.jobs.saturating_sub(1);
                    self.external.last_call = Some(Instant::now());
                    true
                }
            };
            // The project it started on was closed: what it made is kept
            // for that project, and nothing touches the open one.
            if bus.project().id != project {
                let done = self.orphan(project, out);
                self.finish_call(origin, project, reply, tool, args, done, Some(call));
                continue;
            }
            let turn_author = || {
                self.turn
                    .as_ref()
                    .filter(|t| live && !t.stopping())
                    .map(|t| t.author.clone())
            };
            let done = match out {
                JobDone::Done(done) => done,
                JobDone::Stopped => stopped(),
                JobDone::Fetched { item, done, bin } => {
                    let author = match origin {
                        Origin::Turn => turn_author(),
                        Origin::External(_) => Some(external_author()),
                    };
                    tools::web::finish(bus, author.as_ref(), *item, *done, bin)
                }
                JobDone::Propose {
                    commands,
                    reason,
                    excerpts,
                    since,
                    rebase,
                } => {
                    let ctx = match origin {
                        Origin::Turn => {
                            self.turn
                                .as_ref()
                                .filter(|t| live && !t.stopping())
                                .map(|turn| ToolCtx {
                                    bus: &mut *bus,
                                    services,
                                    author: turn.author.clone(),
                                    external: false,
                                    editor: &turn.context,
                                    settings: &turn.settings,
                                    memory: &self.memory,
                                    stop: &turn.stop,
                                })
                        }
                        Origin::External(_) => Some(ToolCtx {
                            bus: &mut *bus,
                            services,
                            author: external_author(),
                            external: true,
                            editor: &self.external.context,
                            settings: &self.external.settings,
                            memory: &self.memory,
                            stop: &self.external.stop,
                        }),
                    };
                    match ctx {
                        Some(mut ctx) => {
                            let mut done =
                                tools::propose_job(&mut ctx, commands, &reason, since, rebase);
                            if !done.result.is_error
                                && let Some(bridge::Content::Text { text }) =
                                    done.result.content.first_mut()
                            {
                                text.push_str("\n\n");
                                text.push_str(&excerpts.join("\n"));
                            }
                            done.excerpts = excerpts;
                            done
                        }
                        None => Done::error("The turn was cancelled."),
                    }
                }
            };
            self.finish_call(origin, project, reply, tool, args, done, Some(call));
        }
    }

    /// What a job came to when the project it started on is no longer
    /// open: what it made is kept for that project, anything else dropped.
    fn orphan(&mut self, project: ProjectId, out: JobDone) -> Done {
        let kept = "Made it, but its project was closed meanwhile; it goes into that \
                    project's media pool when it is opened again.";
        match out {
            JobDone::Done(done) => done,
            JobDone::Stopped => stopped(),
            JobDone::Fetched { item, .. } => {
                self.orphans.push((project, *item));
                Done::text("Kept for its project", kept)
            }
            JobDone::Propose { .. } => {
                Done::error("Its project was closed before the change could be proposed.")
            }
        }
    }

    /// Offer the external set for review once its clients are gone or quiet.
    fn maybe_close_external(&mut self, bus: &mut CommandBus) {
        // A call waiting on the user's approval is still under way.
        let waiting = self
            .held
            .values()
            .any(|(o, _)| matches!(o, Origin::External(_)));
        let ext = &mut self.external;
        let Some(last) = ext.last_call else {
            return;
        };
        if ext.jobs > 0
            || waiting
            || (!ext.connections.is_empty() && last.elapsed() < EXTERNAL_IDLE)
        {
            return;
        }
        ext.last_call = None;
        close_open_sets(bus, &external_author(), None, &mut self.events);
    }

    /// Answer a call made on `project` and show it; `call` is the id its
    /// start was shown under, if it was.
    #[allow(clippy::too_many_arguments)]
    fn finish_call(
        &mut self,
        origin: Origin,
        project: ProjectId,
        reply: Reply,
        tool: String,
        args: Value,
        done: Done,
        call: Option<u64>,
    ) {
        let ok = !done.result.is_error;
        if origin == Origin::Turn
            && let Some(turn) = self.turn.as_mut()
        {
            turn.usage.record(&tool, &done.result);
        }
        // A request for approval keeps its call waiting for the user.
        match done.approval {
            Some(request) => {
                let id = self.next_approval;
                self.next_approval += 1;
                let chat = match origin {
                    Origin::Turn => Some(
                        self.turn
                            .as_ref()
                            .map_or_else(|| self.chat.clone(), |t| t.chat.clone()),
                    ),
                    Origin::External(_) => None,
                };
                self.asked.insert(id, (project, chat.clone()));
                self.approvals.push(PendingApproval {
                    id,
                    request,
                    at: now(),
                    project,
                    chat,
                });
                self.held.insert(id, (origin, reply));
            }
            None => reply.result(done.result),
        }
        let entry = ActivityEntry {
            at: now(),
            tool,
            args: done.recorded.map_or(args, |recorded| *recorded),
            summary: done.summary,
            images: done.images,
            excerpts: done.excerpts,
            ok,
            running: None,
        };
        self.events.push(match call {
            Some(call) => AgentEvent::ToolFinished { call, entry },
            None => AgentEvent::Activity { entry },
        });
        self.events.extend(done.events);
    }
}

/// The row of a call that is still running.
fn started(tool: String, args: Value, summary: String, call: u64) -> ActivityEntry {
    ActivityEntry {
        at: now(),
        tool,
        args,
        summary,
        images: Vec::new(),
        excerpts: Vec::new(),
        ok: true,
        running: Some(call),
    }
}

/// Offer `author`'s open change set for review, or drop it if empty. A
/// `carried` set that got nothing new was offered already.
fn close_open_sets(
    bus: &mut CommandBus,
    author: &Author,
    carried: Option<(ChangeSetId, usize)>,
    events: &mut Vec<AgentEvent>,
) {
    let open: Vec<_> = bus
        .change_sets()
        .iter()
        .filter(|s| &s.author == author && s.state == ChangeSetState::Open)
        .map(|s| (s.id, s.items.len()))
        .collect();
    for (id, len) in open {
        // Closing twice is harmless: `EditorState` closes it again on the event.
        bus.close_change_set(id);
        if len > 0 && carried != Some((id, len)) {
            events.push(AgentEvent::ChangeSetReady { change_set: id });
        }
    }
}

fn is_lost_session(error: &str) -> bool {
    let e = error.to_lowercase();
    e.contains("no conversation found")
        || e.contains("session not found")
        || e.contains("no session")
        || e.contains("thread not found")
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
