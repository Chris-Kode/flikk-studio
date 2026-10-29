//! The agent host as the editor drives it: queue a turn with [`send`], call
//! [`pump`] on the UI thread every frame-ish, read back `AgentEvent`s.
//!
//! One turn = one backend subprocess. Edits the agent makes during the turn
//! land in its open change set (the bus opens it on the first proposal); at
//! the end of the turn the host emits `ChangeSetReady` if it has items, then
//! `TurnDone`.
//!
//! External MCP clients (Claude Code, Codex… running `koo-editor mcp`
//! against the published `bridge.json`) are served between and during
//! turns. They act as the agent's current role with the same permissions,
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
use crate::tools::{self, ApprovalRequest, Done, JobDone, Memory, Step, ToolCtx};
use crate::{prompt, recipes, session};
use koo_core::agent::{ActivityEntry, AgentEvent, AgentSettings, Backend, EditorContext};
use koo_core::bus::{AgentRole, Author, CommandBus};
use koo_core::changeset::ChangeSetState;
use koo_core::command::ToolCategory;
use koo_core::memory::{Recipe, SourceFact, StyleRule};
use koo_core::services::Services;
use serde_json::Value;
use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

/// Quiet time after which an external client's change set goes to review.
pub const EXTERNAL_IDLE: Duration = Duration::from_secs(90);

/// The author external clients' proposals are made under.
pub fn external_author() -> Author {
    Author::Recipe {
        name: "External agent".into(),
    }
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
}

/// A memory or deliver request waiting for the user. Approving it is the
/// UI's job (through the store or deliver crate); the agent was already
/// told it is pending.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingApproval {
    pub id: u64,
    pub request: ApprovalRequest,
    /// Unix seconds.
    pub at: i64,
}

struct Queued {
    message: String,
    context: EditorContext,
    settings: AgentSettings,
}

struct Turn {
    author: Author,
    backend: Backend,
    project: koo_core::ProjectId,
    /// Chat session the turn belongs to; its backend session is resumed.
    chat: String,
    context: EditorContext,
    settings: AgentSettings,
    /// Kept to re-run once without `--resume` if the session is gone.
    spec: TurnSpec,
    running: Option<RunningTurn>,
    /// Off-thread tool jobs still running.
    jobs: usize,
    /// Set when the backend reports the end of the turn.
    finished: Option<Option<String>>,
    cancelled: bool,
    saw_text: bool,
    /// Tool groups the chat lists, fixed when it started (see
    /// `tools::groups`).
    groups: BTreeSet<ToolCategory>,
    usage: Usage,
}

type JobResult = (Origin, Reply, String, Value, JobDone);

pub struct AgentHost {
    bridge: Option<BridgeServer>,
    role: AgentRole,
    /// Chat session whose backend conversation the next turn continues.
    chat: String,
    memory: Memory,
    queued: Option<Queued>,
    turn: Option<Turn>,
    events: Vec<AgentEvent>,
    jobs_tx: Sender<JobResult>,
    jobs_rx: Receiver<JobResult>,
    approvals: Vec<PendingApproval>,
    next_approval: u64,
    /// What became of approval requests, told to the agent on its next turn.
    outcomes: Vec<String>,
    external: External,
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
            role: AgentRole::Editor,
            chat: session::MAIN_CHAT.to_owned(),
            memory: Memory::default(),
            queued: None,
            turn: None,
            events: Vec::new(),
            jobs_tx,
            jobs_rx,
            approvals: Vec::new(),
            next_approval: 1,
            outcomes: Vec::new(),
            external: External::default(),
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

    pub fn is_busy(&self) -> bool {
        self.queued.is_some() || self.turn.is_some()
    }

    /// Editor or Builder: a different prompt, tool set and backend session.
    /// Takes effect on the next turn.
    pub fn set_role(&mut self, role: AgentRole) {
        self.role = role;
    }

    pub fn role(&self) -> AgentRole {
        self.role
    }

    /// Continue chat session `chat` from the next turn on.
    pub fn set_chat(&mut self, chat: impl Into<String>) {
        self.chat = chat.into();
    }

    /// Recipes available as `/name` commands (the store seeds built-ins).
    pub fn set_recipes(&mut self, recipes: Vec<Recipe>) {
        self.memory.recipes = recipes;
    }

    /// Style rules and source facts; rules apply by scope, facts are fetched
    /// by range inside tools.
    pub fn set_memory(&mut self, rules: Vec<StyleRule>, facts: Vec<SourceFact>) {
        self.memory.rules = rules;
        self.memory.facts = facts;
    }

    /// Queue a turn; it starts on the next [`pump`](Self::pump).
    pub fn send(&mut self, message: String, context: EditorContext, settings: &AgentSettings) {
        if self.is_busy() {
            self.events.push(AgentEvent::Error {
                message: "The agent is still working on the last message".into(),
            });
            return;
        }
        self.external.context = context.clone();
        self.external.settings = settings.clone();
        self.queued = Some(Queued {
            message,
            context,
            settings: settings.clone(),
        });
    }

    /// Stop the turn: kill the subprocess. The partial change set is still
    /// offered for review on the next pump.
    pub fn cancel(&mut self) {
        self.queued = None;
        if let Some(turn) = self.turn.as_mut() {
            turn.cancelled = true;
            if let Some(r) = &turn.running {
                r.kill();
            }
        }
    }

    pub fn pending_approvals(&self) -> &[PendingApproval] {
        &self.approvals
    }

    /// Remove a request once the user approved or denied it.
    pub fn resolve_approval(&mut self, id: u64) -> Option<PendingApproval> {
        let ix = self.approvals.iter().position(|a| a.id == id)?;
        Some(self.approvals.remove(ix))
    }

    /// Tell the agent, at the start of its next turn, what became of an
    /// approval request (`"approved: Style rule: …"`).
    pub fn report_outcome(&mut self, outcome: String) {
        self.outcomes.push(outcome);
    }

    pub fn pump(&mut self, bus: &mut CommandBus, services: &Services) -> Vec<AgentEvent> {
        if self.turn.is_none()
            && let Some(q) = self.queued.take()
        {
            self.start(q, bus);
        }
        self.serve_calls(bus, services);
        self.drain_jobs(bus, services);
        self.drain_backend();
        self.maybe_finish(bus);
        self.maybe_close_external(bus);
        std::mem::take(&mut self.events)
    }

    // ------------------------------------------------------------- turns

    fn start(&mut self, q: Queued, bus: &mut CommandBus) {
        let author = Author::Agent { role: self.role };
        // A set left open by an earlier turn that never finished.
        close_open_sets(bus, &author, &mut self.events);
        let project = bus.project();
        let enabled: Vec<Recipe> = self
            .memory
            .recipes
            .iter()
            .filter(|r| {
                q.settings.enabled_recipes.is_empty() || q.settings.enabled_recipes.contains(&r.id)
            })
            .cloned()
            .collect();
        let (recipe, text) = match recipes::expand(&q.message, &enabled) {
            Some((name, prompt)) => (Some(name), prompt),
            None => (None, q.message.clone()),
        };
        let mut message = prompt::turn_message(
            &text,
            project,
            &q.context,
            &self.memory.rules,
            recipe.as_deref(),
        );
        if !self.outcomes.is_empty() {
            let outcomes = std::mem::take(&mut self.outcomes).join("\n");
            message = format!("<approvals>\n{outcomes}\n</approvals>\n\n{message}");
        }
        let system_prompt = prompt::system_prompt(self.role, &q.settings);
        let backend = q.settings.backend;
        let resume = session::load(project.id, &self.chat, backend, self.role);
        let picked = groups::pick(&text, project, &q.context, self.role);
        // A conversation keeps the list it started with, so its cached
        // prompt stays valid; a new one starts from this message's picks.
        let key = session::key(project.id, &self.chat, backend, self.role);
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
                self.fail_now(format!("Could not start the agent: {e}"));
                return;
            }
        };

        let spec = TurnSpec {
            system_prompt,
            message,
            resume,
            model: q.settings.models.get(backend).clone(),
            mcp,
            workdir,
            run_dir,
        };
        let running = match backend::start(backend::adapter(backend), spec.clone()) {
            Ok(r) => r,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&spec.run_dir);
                self.fail_now(format!("Could not start {}: {e}", backend.label()));
                return;
            }
        };
        self.events.push(AgentEvent::Progress {
            label: recipe.map_or_else(|| "Thinking".to_owned(), |r| format!("Running /{r}")),
            fraction: None,
        });
        self.turn = Some(Turn {
            author,
            backend,
            project: project.id,
            chat: self.chat.clone(),
            context: q.context,
            settings: q.settings,
            spec,
            running: Some(running),
            jobs: 0,
            finished: None,
            cancelled: false,
            saw_text: false,
            usage: Usage::new(picked, groups.clone()),
            groups,
        });
    }

    fn fail_now(&mut self, message: String) {
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
        for event in running.events.try_iter() {
            match event {
                BackendEvent::Session(id) => session::store(
                    turn.project,
                    &turn.chat,
                    turn.backend,
                    role_of(&turn.author),
                    Some(&id),
                ),
                BackendEvent::TextDelta(text) => {
                    turn.saw_text = true;
                    if !turn.cancelled {
                        self.events.push(AgentEvent::TextDelta { text });
                    }
                }
                BackendEvent::Finished { error } => {
                    turn.finished = Some(error);
                    break;
                }
            }
        }
        // Resume failed because the backend forgot the session: forget it
        // too and run the turn again fresh.
        let lost = matches!(&turn.finished, Some(Some(e)) if is_lost_session(e));
        if lost && turn.spec.resume.is_some() && !turn.saw_text && !turn.cancelled {
            session::store(
                turn.project,
                &turn.chat,
                turn.backend,
                role_of(&turn.author),
                None,
            );
            let mut spec = turn.spec.clone();
            spec.resume = None;
            match backend::start(backend::adapter(turn.backend), spec.clone()) {
                Ok(r) => {
                    turn.spec = spec;
                    turn.running = Some(r);
                    turn.finished = None;
                }
                Err(e) => turn.finished = Some(Some(e.to_string())),
            }
        }
    }

    fn maybe_finish(&mut self, bus: &mut CommandBus) {
        let Some(turn) = self.turn.as_ref() else {
            return;
        };
        let done = turn.cancelled || (turn.finished.is_some() && turn.jobs == 0);
        if !done {
            return;
        }
        let turn = self.turn.take().unwrap();
        if turn.cancelled {
            if let Some(r) = &turn.running {
                r.kill();
            }
        } else if let Some(Some(error)) = &turn.finished {
            self.events.push(AgentEvent::Error {
                message: error.clone(),
            });
        }
        close_open_sets(bus, &turn.author, &mut self.events);
        turn.usage
            .log(&turn.settings, role_of(&turn.author), &turn.context, now());
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
                    }
                }
                (Origin::Turn, IncomingKind::List) => match self.turn.as_mut() {
                    Some(turn) => {
                        let list = tools::list(bus, role_of(&turn.author), Some(&turn.groups));
                        turn.usage.listed = list.len();
                        turn.usage.listed_bytes = list.iter().map(|t| t.to_string().len()).sum();
                        reply.tools(list);
                    }
                    None => reply.tools(tools::list(bus, self.role, None)),
                },
                (Origin::External(id), IncomingKind::List) => {
                    self.external.connections.insert(id);
                    reply.tools(tools::list(bus, self.role, None));
                }
                (Origin::Turn, IncomingKind::Call { tool, args }) => {
                    let Some(turn) = self.turn.as_mut().filter(|t| !t.cancelled) else {
                        reply.result(bridge::ToolResult::error("No turn is running."));
                        continue;
                    };
                    let via_run = tool == groups::RUN_TOOL;
                    let (tool, args) = groups::unwrap_run(tool, args);
                    turn.usage.calls.push((tool.clone(), via_run));
                    if tool == groups::LOAD_TOOLS {
                        let done = groups::load(
                            bus,
                            role_of(&turn.author),
                            &turn.groups,
                            &mut turn.usage.loaded,
                            &args,
                        );
                        self.finish_call(reply, tool, args, done);
                        continue;
                    }
                    let mut ctx = ToolCtx {
                        bus,
                        services,
                        author: turn.author.clone(),
                        role: role_of(&turn.author),
                        external: false,
                        editor: &turn.context,
                        settings: &turn.settings,
                        memory: &self.memory,
                    };
                    match tools::serve(&mut ctx, &tool, &args) {
                        Step::Done(done) => self.finish_call(reply, tool, args, *done),
                        Step::Job(job) => {
                            turn.jobs += 1;
                            self.spawn_job(origin, reply, tool, args, job);
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
                        role: self.role,
                        external: true,
                        editor: &self.external.context,
                        settings: &self.external.settings,
                        memory: &self.memory,
                    };
                    match tools::serve(&mut ctx, &tool, &args) {
                        Step::Done(done) => self.finish_call(reply, tool, args, *done),
                        Step::Job(job) => {
                            self.external.jobs += 1;
                            self.spawn_job(origin, reply, tool, args, job);
                        }
                    }
                }
            }
        }
    }

    fn spawn_job(&self, origin: Origin, reply: Reply, tool: String, args: Value, job: tools::Job) {
        let tx = self.jobs_tx.clone();
        std::thread::spawn(move || {
            let out = job();
            let _ = tx.send((origin, reply, tool, args, out));
        });
    }

    fn drain_jobs(&mut self, bus: &mut CommandBus, services: &Services) {
        let results: Vec<_> = self.jobs_rx.try_iter().collect();
        for (origin, reply, tool, args, out) in results {
            match origin {
                Origin::Turn => {
                    if let Some(turn) = self.turn.as_mut() {
                        turn.jobs = turn.jobs.saturating_sub(1);
                    }
                }
                Origin::External(_) => {
                    self.external.jobs = self.external.jobs.saturating_sub(1);
                    self.external.last_call = Some(Instant::now());
                }
            }
            let done = match out {
                JobDone::Done(done) => done,
                JobDone::Propose {
                    commands,
                    reason,
                    excerpts,
                } => {
                    let ctx = match origin {
                        Origin::Turn => {
                            self.turn
                                .as_ref()
                                .filter(|t| !t.cancelled)
                                .map(|turn| ToolCtx {
                                    bus: &mut *bus,
                                    services,
                                    author: turn.author.clone(),
                                    role: role_of(&turn.author),
                                    external: false,
                                    editor: &turn.context,
                                    settings: &turn.settings,
                                    memory: &self.memory,
                                })
                        }
                        Origin::External(_) => Some(ToolCtx {
                            bus: &mut *bus,
                            services,
                            author: external_author(),
                            role: self.role,
                            external: true,
                            editor: &self.external.context,
                            settings: &self.external.settings,
                            memory: &self.memory,
                        }),
                    };
                    match ctx {
                        Some(mut ctx) => {
                            let mut done = tools::propose(&mut ctx, commands, &reason);
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
            self.finish_call(reply, tool, args, done);
        }
    }

    /// Offer the external set for review once its clients are gone or quiet.
    fn maybe_close_external(&mut self, bus: &mut CommandBus) {
        let ext = &mut self.external;
        let Some(last) = ext.last_call else {
            return;
        };
        if ext.jobs > 0 || (!ext.connections.is_empty() && last.elapsed() < EXTERNAL_IDLE) {
            return;
        }
        ext.last_call = None;
        close_open_sets(bus, &external_author(), &mut self.events);
    }

    fn finish_call(&mut self, reply: Reply, tool: String, args: Value, done: Done) {
        let ok = !done.result.is_error;
        reply.result(done.result);
        self.events.push(AgentEvent::Activity {
            entry: ActivityEntry {
                at: now(),
                tool,
                args,
                summary: done.summary,
                images: done.images,
                excerpts: done.excerpts,
                ok,
            },
        });
        self.events.extend(done.events);
        if let Some(request) = done.approval {
            self.approvals.push(PendingApproval {
                id: self.next_approval,
                request,
                at: now(),
            });
            self.next_approval += 1;
        }
    }
}

/// Offer `author`'s open change set for review, or drop it if empty.
fn close_open_sets(bus: &mut CommandBus, author: &Author, events: &mut Vec<AgentEvent>) {
    let open: Vec<_> = bus
        .change_sets()
        .iter()
        .filter(|s| &s.author == author && s.state == ChangeSetState::Open)
        .map(|s| (s.id, s.items.is_empty()))
        .collect();
    for (id, empty) in open {
        // Closing twice is harmless: `EditorState` closes it again on the event.
        bus.close_change_set(id);
        if !empty {
            events.push(AgentEvent::ChangeSetReady { change_set: id });
        }
    }
}

fn role_of(author: &Author) -> AgentRole {
    match author {
        Author::Agent { role } => *role,
        _ => AgentRole::Editor,
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
