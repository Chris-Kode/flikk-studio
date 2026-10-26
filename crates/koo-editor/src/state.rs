//! Shared editor state. One entity per window; panels observe it and call
//! its methods instead of touching the bus directly, so every change goes
//! through the same path whether it came from a button or the agent.

use crate::agent_link::AgentLink;
use crate::persistence;
use gpui_kit::*;
use koo_core::agent::*;
use koo_core::bus::{AcceptReport, AgentRole, Author, CommandBus, Outcome};
use koo_core::command::Command;
use koo_core::model::Project;
use koo_core::services::{AnalysisService, MediaService, Services};
use koo_core::{ChangeItemId, ChangeSetId, Time, TimeRange};
use std::sync::Arc;
use std::time::Duration;

/// How often agent output and tool calls are drained on the UI thread.
const AGENT_PUMP: Duration = Duration::from_millis(33);

pub struct EditorState {
    pub bus: CommandBus,
    pub services: Services,
    pub workspace: Workspace,
    pub playhead: Time,
    pub in_out: Option<TimeRange>,
    pub selection: Selection,
    pub playing: bool,
    pub settings: AgentSettings,
    pub chat: Vec<ChatMessage>,
    pub activity: Vec<ActivityEntry>,
    /// Change set whose draft the viewer plays instead of the committed project.
    pub preview: Option<ChangeSetId>,
    /// Last error or notice for the status line; cleared on the next action.
    pub status: Option<String>,
    agent: AgentLink,
    _pump: Task<()>,
    // --- chat
    /// Mirrors the role last handed to the agent host.
    agent_role: AgentRole,
}

impl EditorState {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let persistence::Loaded {
            project,
            history,
            settings,
            ..
        } = persistence::load_session();
        let mut bus = CommandBus::with_history(project, history);
        bus.set_permissions(settings.permissions.clone());
        let pump = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(AGENT_PUMP).await;
                if this.update(cx, |this, cx| this.pump_agent(cx)).is_err() {
                    break;
                }
            }
        });
        let mut this = Self {
            bus,
            services: services(),
            workspace: Workspace::default(),
            playhead: Time::ZERO,
            in_out: None,
            selection: Selection::default(),
            playing: false,
            settings,
            chat: Vec::new(),
            activity: Vec::new(),
            preview: None,
            status: persistence::take_notice(),
            agent: AgentLink::new(),
            _pump: pump,
            agent_role: AgentRole::Editor,
        };
        this.load_agent_memory();
        this
    }

    /// Hand the agent its recipes, the approved rules of the selected style
    /// profile and the facts about this project's sources.
    fn load_agent_memory(&mut self) {
        use koo_core::memory::{FactStatus, RuleStatus};
        use koo_store::{FactFilter, RuleFilter};
        let recipes = persistence::with_store(|s| s.list_recipes()).unwrap_or_default();
        let rules = self
            .settings
            .style_profile
            .and_then(|profile| {
                let filter = RuleFilter {
                    status: Some(RuleStatus::Approved),
                    ..Default::default()
                };
                persistence::with_store(|s| s.rules(profile, &filter))
            })
            .unwrap_or_default();
        let fingerprints: Vec<_> = self
            .bus
            .project()
            .media
            .iter()
            .filter_map(|m| m.fingerprint.clone())
            .collect();
        let facts = fingerprints
            .into_iter()
            .flat_map(|fingerprint| {
                let filter = FactFilter {
                    fingerprint: Some(fingerprint),
                    ..Default::default()
                };
                persistence::with_store(|s| s.facts(&filter)).unwrap_or_default()
            })
            .filter(|f| f.status != FactStatus::Suspended)
            .collect();
        self.agent.set_recipes(recipes);
        self.agent.set_memory(rules, facts);
    }

    pub fn context(&self) -> EditorContext {
        EditorContext {
            workspace: self.workspace,
            playhead: self.playhead,
            in_out: self.in_out,
            selection: self.selection.clone(),
        }
    }

    /// What the viewer shows: the previewed change set's draft, or the
    /// committed project.
    pub fn display_project(&self) -> &Project {
        self.preview
            .and_then(|id| self.bus.change_set(id))
            .and_then(|set| set.draft.as_ref())
            .unwrap_or(self.bus.project())
    }

    pub fn set_preview(&mut self, preview: Option<ChangeSetId>, cx: &mut Context<Self>) {
        self.preview = preview;
        cx.notify();
    }

    // ------------------------------------------------------------ hand edits

    pub fn execute(&mut self, command: Command, cx: &mut Context<Self>) -> Option<Outcome> {
        let result = self.bus.execute(command, Author::User, "");
        self.finish(result, cx)
    }

    pub fn execute_batch(
        &mut self,
        commands: Vec<Command>,
        label: &str,
        cx: &mut Context<Self>,
    ) -> Option<Outcome> {
        let result = self.bus.execute_batch(commands, label);
        self.finish(result, cx)
    }

    pub fn undo(&mut self, cx: &mut Context<Self>) {
        let result = self.bus.undo();
        self.finish(result, cx);
    }

    pub fn redo(&mut self, cx: &mut Context<Self>) {
        let result = self.bus.redo();
        self.finish(result, cx);
    }

    pub fn accept(
        &mut self,
        set: ChangeSetId,
        items: Option<&[ChangeItemId]>,
        cx: &mut Context<Self>,
    ) -> Option<AcceptReport> {
        let result = self.bus.accept(set, items);
        self.finish(result, cx)
    }

    pub fn reject(
        &mut self,
        set: ChangeSetId,
        items: Option<&[ChangeItemId]>,
        cx: &mut Context<Self>,
    ) {
        self.bus.reject(set, items);
        cx.notify();
    }

    fn finish<T>(&mut self, result: koo_core::Result<T>, cx: &mut Context<Self>) -> Option<T> {
        self.status = result.as_ref().err().map(|e| e.to_string());
        if result.is_ok() {
            persistence::autosave(&self.bus);
        }
        cx.notify();
        result.ok()
    }

    // ------------------------------------------------------------- navigation

    pub fn set_workspace(&mut self, workspace: Workspace, cx: &mut Context<Self>) {
        self.workspace = workspace;
        cx.notify();
    }

    pub fn seek(&mut self, t: Time, cx: &mut Context<Self>) {
        self.playhead = t.max(Time::ZERO);
        cx.notify();
    }

    pub fn select_clips(&mut self, clips: Vec<koo_core::ClipId>, cx: &mut Context<Self>) {
        self.selection.clips = clips;
        cx.notify();
    }

    // ------------------------------------------------------------------ agent

    pub fn agent_busy(&self) -> bool {
        self.agent.is_busy()
    }

    pub fn send_chat(&mut self, text: String, cx: &mut Context<Self>) {
        self.chat.push(ChatMessage {
            role: ChatRole::User,
            text: text.clone(),
            choices: vec![],
            change_set: None,
            at: now(),
        });
        let context = self.context();
        self.agent.send(text, context, &self.settings);
        cx.notify();
    }

    pub fn cancel_agent(&mut self, cx: &mut Context<Self>) {
        self.agent.cancel();
        cx.notify();
    }

    pub fn set_settings(&mut self, settings: AgentSettings, cx: &mut Context<Self>) {
        self.bus.set_permissions(settings.permissions.clone());
        persistence::save_settings(&settings);
        self.settings = settings;
        self.load_agent_memory();
        cx.notify();
    }

    fn pump_agent(&mut self, cx: &mut Context<Self>) {
        let events = self.agent.pump(&mut self.bus, &self.services);
        if events.is_empty() {
            return;
        }
        for event in events {
            match event {
                AgentEvent::TextDelta { text } => match self.chat.last_mut() {
                    Some(m)
                        if m.role == ChatRole::Agent
                            && m.change_set.is_none()
                            && m.choices.is_empty() =>
                    {
                        m.text.push_str(&text)
                    }
                    _ => self.push_agent(text, vec![], None),
                },
                AgentEvent::Question { text, choices } => self.push_agent(text, choices, None),
                AgentEvent::ChangeSetReady { change_set } => {
                    self.bus.close_change_set(change_set);
                    let title = self
                        .bus
                        .change_set(change_set)
                        .map(|s| s.title.clone())
                        .unwrap_or_default();
                    self.push_agent(title, vec![], Some(change_set));
                }
                AgentEvent::Activity { entry } => self.activity.push(entry),
                AgentEvent::Progress { .. } | AgentEvent::TurnDone => {}
                AgentEvent::Error { message } => self.status = Some(message),
            }
        }
        cx.notify();
    }

    fn push_agent(&mut self, text: String, choices: Vec<String>, change_set: Option<ChangeSetId>) {
        self.chat.push(ChatMessage {
            role: ChatRole::Agent,
            text,
            choices,
            change_set,
            at: now(),
        });
    }
}

/// The engines, wired once per window. Media is optional: without FFmpeg
/// the editor still opens, and panels degrade quietly.
fn services() -> Services {
    let media = match koo_media::FfmpegMedia::new() {
        Ok(m) => Some(Arc::new(m) as Arc<dyn MediaService>),
        Err(e) => {
            eprintln!("media engine unavailable: {e}");
            None
        }
    };
    Services {
        media,
        analysis: Some(Arc::new(koo_analysis::LocalAnalysis::default()) as Arc<dyn AnalysisService>),
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// --- chat

impl EditorState {
    pub fn agent_role(&self) -> AgentRole {
        self.agent_role
    }

    pub fn set_agent_role(&mut self, role: AgentRole, cx: &mut Context<Self>) {
        self.agent_role = role;
        self.agent.set_role(role);
        cx.notify();
    }

    /// Memory and deliver requests waiting for the user (PRD "Agent
    /// configuration": these categories need approval per call).
    pub fn pending_approvals(&self) -> &[koo_agent::PendingApproval] {
        self.agent.pending_approvals()
    }

    pub fn deny_approval(&mut self, id: u64, cx: &mut Context<Self>) {
        self.agent.resolve_approval(id);
        cx.notify();
    }

    /// Perform an approved request and take it off the list.
    pub fn approve(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(pending) = self.agent.resolve_approval(id) else {
            return;
        };
        let result = self.perform(&pending.request);
        self.status = result.err();
        self.load_agent_memory();
        cx.notify();
    }

    /// The user's recipes, for `/` completion and the quick-command bar.
    pub fn recipes(&self) -> Vec<koo_core::memory::Recipe> {
        persistence::with_store(|s| s.list_recipes()).unwrap_or_default()
    }

    fn perform(&self, request: &koo_agent::tools::ApprovalRequest) -> Result<(), String> {
        use koo_core::memory::*;
        use serde_json::Value;
        let args = &request.args;
        let str_of = |key: &str| args.get(key).and_then(Value::as_str).unwrap_or_default();
        let saved = match request.tool.as_str() {
            "save_recipe" => {
                let recipe = Recipe {
                    id: koo_core::RecipeId::new(),
                    name: str_of("name").into(),
                    description: str_of("description").into(),
                    params: serde_json::from_value(args["params"].clone()).unwrap_or_default(),
                    prompt: str_of("prompt").into(),
                    builtin: false,
                };
                persistence::with_store(|s| s.save_recipe(&recipe))
            }
            "propose_rule" => {
                let profile = self
                    .settings
                    .style_profile
                    .ok_or("Pick a style profile before saving rules")?;
                let rule = StyleRule {
                    id: koo_core::RuleId::new(),
                    text: str_of("text").into(),
                    scope: match str_of("scope") {
                        "project" => RuleScope::Project {
                            project: self.bus.project().id,
                        },
                        "next_video" => RuleScope::NextVideo,
                        _ => RuleScope::Global,
                    },
                    status: RuleStatus::Approved,
                    supersedes: None,
                    created_at: now(),
                    evidence: serde_json::from_value(args["evidence"].clone()).unwrap_or_default(),
                };
                persistence::with_store(|s| s.save_rule(profile, &rule))
            }
            "add_source_fact" => {
                let secs = |v: &Value| v.as_f64().map(Time::from_secs_f64);
                let range = args
                    .get("source_range")
                    .and_then(|r| Some(TimeRange::new(secs(&r["start"])?, secs(&r["end"])?)));
                let fact = SourceFact {
                    id: koo_core::FactId::new(),
                    fingerprint: serde_json::from_value(args["fingerprint"].clone())
                        .map_err(|e| e.to_string())?,
                    kind: serde_json::from_value(args["kind"].clone())
                        .map_err(|e| e.to_string())?,
                    text: str_of("text").into(),
                    range,
                    status: FactStatus::Confirmed,
                };
                persistence::with_store(|s| s.save_fact(&fact))
            }
            _ => return Err(format!("{} isn't connected yet", request.summary)),
        };
        saved.ok_or_else(|| format!("Couldn't save: {}", request.summary))
    }
}
