//! Shared editor state. One entity per window; panels observe it and call
//! its methods instead of touching the bus directly, so every change goes
//! through the same path whether it came from a button or the agent.

use crate::agent_link::{AgentLink, approve};
use crate::persistence;
use gpui_kit::*;
use koo_agent::PendingApproval;
use koo_core::agent::*;
use koo_core::bus::{AcceptReport, AgentRole, Author, CommandBus, Outcome};
use koo_core::changeset::ChangeItem;
use koo_core::command::{Command, Warning};
use koo_core::model::Project;
use koo_core::services::{AnalysisService, MediaService, Services};
use koo_core::{ChangeItemId, ChangeSetId, Time, TimeRange};
use std::sync::Arc;
use std::time::{Duration, Instant};

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
    // --- panels
    /// Pool item loaded in the viewer as source (double-click in the pool).
    pub source_media: Option<koo_core::MediaId>,
    /// Caption styles, loudness and ducking defaults until profiles are wired.
    pub presets: koo_core::memory::Presets,
    /// Index into `presets.caption_styles` used for new captions.
    pub caption_style: usize,
    // --- timeline/viewer
    /// Shuttle speed while playing: 1.0 is real time, negative plays backwards (JKL).
    pub play_speed: f64,
    /// Change item picked on the timeline overlay; the chat panel reveals it.
    pub focused_change: Option<(ChangeSetId, ChangeItemId)>,
    // --- chat
    /// Agent requests waiting for, or settled by, the user's approval.
    pub approvals: Vec<ApprovalCard>,
    /// Renders approved in the chat. Clones share the queue.
    pub render_queue: koo_deliver::RenderQueue,
    clock: Option<Task<()>>,
    agent: AgentLink,
    _pump: Task<()>,
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
            source_media: None,
            presets: koo_core::memory::Presets::default(),
            caption_style: 0,
            play_speed: 1.0,
            focused_change: None,
            approvals: Vec::new(),
            render_queue: koo_deliver::RenderQueue::new(),
            clock: None,
            agent: AgentLink::new(),
            _pump: pump,
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

    // --- panels

    pub fn set_source_media(&mut self, media: Option<koo_core::MediaId>, cx: &mut Context<Self>) {
        self.source_media = media;
        cx.notify();
    }

    pub fn select_words(
        &mut self,
        words: Option<(koo_core::MediaId, TimeRange)>,
        cx: &mut Context<Self>,
    ) {
        self.selection.words = words;
        cx.notify();
    }

    /// The caption style new captions use.
    pub fn active_caption_style(&self) -> koo_core::memory::CaptionStyle {
        self.presets
            .caption_styles
            .get(self.caption_style)
            .cloned()
            .unwrap_or_default()
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
        self.sync_approvals();
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

// --- chat

/// An approval request in the thread, under the agent message it came with.
#[derive(Clone, Debug)]
pub struct ApprovalCard {
    pub approval: PendingApproval,
    /// Index into `EditorState::chat`.
    pub message: usize,
    /// `None` while it waits for the user.
    pub outcome: Option<ApprovalOutcome>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ApprovalOutcome {
    Working,
    Done(String),
    Rendering { job: koo_core::JobId, file: String },
    Failed(String),
    Denied,
}

impl EditorState {
    /// Give each new request a card under the current agent message.
    fn sync_approvals(&mut self) {
        let new: Vec<PendingApproval> = self
            .agent
            .pending_approvals()
            .iter()
            .filter(|p| !self.approvals.iter().any(|c| c.approval.id == p.id))
            .cloned()
            .collect();
        for approval in new {
            if self.chat.last().is_none_or(|m| m.role != ChatRole::Agent) {
                self.push_agent(String::new(), vec![], None);
            }
            self.approvals.push(ApprovalCard {
                approval,
                message: self.chat.len() - 1,
                outcome: None,
            });
        }
    }

    /// Carry out a request (store, render queue, export), then take it off
    /// the agent's list and tell the agent on its next turn.
    pub fn approve(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(ix) = self
            .approvals
            .iter()
            .position(|c| c.approval.id == id && c.outcome.is_none())
        else {
            return;
        };
        let request = self.approvals[ix].approval.request.clone();
        let summary = request.summary.clone();
        self.agent.resolve_approval(id);
        let mut settings = self.settings.clone();
        let started = approve::start(
            &request,
            approve::Inputs {
                project: self.bus.project(),
                caption_styles: &self.presets.caption_styles,
                queue: &self.render_queue,
                settings: &mut settings,
            },
        );
        if settings != self.settings {
            self.set_settings(settings, cx);
        } else {
            self.load_agent_memory();
        }
        let outcome = match started {
            Ok(approve::Started::Done(line)) => {
                self.agent.report_outcome(format!("approved: {summary}"));
                ApprovalOutcome::Done(line)
            }
            Ok(approve::Started::Render(job, path)) => {
                self.agent.report_outcome(format!(
                    "approved: {summary}; rendering to {}",
                    path.display()
                ));
                ApprovalOutcome::Rendering {
                    job,
                    file: approve::file_name(&path),
                }
            }
            Ok(approve::Started::Background(work)) => {
                let task = cx.background_executor().spawn(async move { work() });
                cx.spawn(async move |this, cx| {
                    let result = task.await;
                    let _ = this.update(cx, |this, cx| {
                        let outcome = match result {
                            Ok(file) => {
                                this.agent
                                    .report_outcome(format!("approved: {summary}; wrote {file}"));
                                ApprovalOutcome::Done(file)
                            }
                            Err(e) => {
                                this.agent.report_outcome(format!(
                                    "approved but failed: {summary}: {e:#}"
                                ));
                                ApprovalOutcome::Failed(format!("{e:#}"))
                            }
                        };
                        if let Some(card) = this.approvals.iter_mut().find(|c| c.approval.id == id)
                        {
                            card.outcome = Some(outcome);
                        }
                        cx.notify();
                    });
                })
                .detach();
                ApprovalOutcome::Working
            }
            Err(e) => {
                self.agent
                    .report_outcome(format!("approved but failed: {summary}: {e:#}"));
                ApprovalOutcome::Failed(format!("{e:#}"))
            }
        };
        self.approvals[ix].outcome = Some(outcome);
        cx.notify();
    }

    pub fn deny(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(card) = self
            .approvals
            .iter_mut()
            .find(|c| c.approval.id == id && c.outcome.is_none())
        else {
            return;
        };
        card.outcome = Some(ApprovalOutcome::Denied);
        let summary = card.approval.request.summary.clone();
        self.agent.resolve_approval(id);
        self.agent.report_outcome(format!("denied: {summary}"));
        cx.notify();
    }

    /// A render started from the chat, as the queue sees it now.
    pub fn render_job(&self, job: koo_core::JobId) -> Option<koo_deliver::RenderJob> {
        self.render_queue.jobs().into_iter().find(|j| j.id == job)
    }

    pub fn agent_role(&self) -> AgentRole {
        self.agent.role()
    }

    /// Editor or Builder; the agent switches from its next turn.
    pub fn set_agent_role(&mut self, role: AgentRole, cx: &mut Context<Self>) {
        self.agent.set_role(role);
        cx.notify();
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

// --- timeline/viewer
impl EditorState {
    /// Like [`Self::execute`], but reports plan warnings (protected ranges)
    /// in the status line.
    pub fn edit(&mut self, command: Command, cx: &mut Context<Self>) -> Option<Outcome> {
        let outcome = self.execute(command, cx);
        self.report_warnings(outcome.as_ref());
        outcome
    }

    /// Several commands as one undo step, reporting warnings like [`Self::edit`].
    pub fn edit_batch(
        &mut self,
        commands: Vec<Command>,
        label: &str,
        cx: &mut Context<Self>,
    ) -> Option<Outcome> {
        let outcome = self.execute_batch(commands, label, cx);
        self.report_warnings(outcome.as_ref());
        outcome
    }

    fn report_warnings(&mut self, outcome: Option<&Outcome>) {
        let warnings = match outcome {
            Some(Outcome::Applied { warnings }) | Some(Outcome::Proposed { warnings, .. }) => {
                warnings
            }
            None => return,
        };
        let text: Vec<String> = warnings.iter().map(warning_text).collect();
        if !text.is_empty() {
            self.status = Some(text.join(" · "));
        }
    }

    /// The change item picked on the timeline, as `(set, item)`.
    pub fn focused_change_item(&self) -> Option<(ChangeSetId, ChangeItemId)> {
        self.focused_change
    }

    /// The picked change item itself, if it still exists.
    pub fn focused_change(&self) -> Option<&ChangeItem> {
        let (set, item) = self.focused_change?;
        self.bus.change_set(set)?.item(item)
    }

    pub fn focus_change_item(
        &mut self,
        item: Option<(ChangeSetId, ChangeItemId)>,
        cx: &mut Context<Self>,
    ) {
        if self.focused_change != item {
            self.focused_change = item;
            cx.notify();
        }
    }

    pub fn set_in_out(&mut self, in_out: Option<TimeRange>, cx: &mut Context<Self>) {
        self.in_out = in_out;
        cx.notify();
    }

    /// Mark in at the playhead, keeping the out point when it is still after it.
    pub fn mark_in(&mut self, cx: &mut Context<Self>) {
        let t = self.playhead;
        let end = match self.in_out {
            Some(r) if r.end > t => r.end,
            _ => self.bus.project().timeline.duration().max(t),
        };
        self.set_in_out(Some(TimeRange::new(t, end)), cx);
    }

    /// Mark out at the playhead, keeping the in point when it is still before it.
    pub fn mark_out(&mut self, cx: &mut Context<Self>) {
        let t = self.playhead;
        let start = match self.in_out {
            Some(r) if r.start < t => r.start,
            _ => Time::ZERO,
        };
        self.set_in_out(Some(TimeRange::new(start, t)), cx);
    }

    pub fn toggle_play(&mut self, cx: &mut Context<Self>) {
        let playing = !self.playing;
        self.play_speed = 1.0;
        self.set_playing(playing, cx);
    }

    /// JKL: `speed` 0 stops; otherwise play at that rate (negative = reverse).
    pub fn shuttle(&mut self, speed: f64, cx: &mut Context<Self>) {
        if speed == 0.0 {
            self.set_playing(false, cx);
        } else {
            self.play_speed = speed;
            self.clock = None;
            self.set_playing(true, cx);
        }
    }

    pub fn set_playing(&mut self, playing: bool, cx: &mut Context<Self>) {
        self.playing = playing;
        if !playing {
            self.clock = None;
            cx.notify();
            return;
        }
        let end = self.display_project().timeline.duration();
        if self.play_speed > 0.0 && self.playhead >= end {
            self.playhead = Time::ZERO;
        }
        if self.clock.is_none() {
            self.clock = Some(cx.spawn(async move |this, cx| {
                let mut last = (Instant::now(), None::<Time>);
                loop {
                    cx.background_executor().timer(PLAY_TICK).await;
                    let alive = this.update(cx, |this, cx| this.tick(&mut last, cx));
                    if !matches!(alive, Ok(true)) {
                        break;
                    }
                }
            }));
        }
        cx.notify();
    }

    /// Advance the playhead by the wall time since the last tick. A seek in
    /// between (the playhead moved elsewhere) restarts from the new position.
    /// Returns false once playback stopped.
    fn tick(&mut self, last: &mut (Instant, Option<Time>), cx: &mut Context<Self>) -> bool {
        if !self.playing {
            return false;
        }
        let now = Instant::now();
        let elapsed = now.duration_since(last.0).as_secs_f64() * self.play_speed;
        *last = (now, Some(self.playhead));
        let Some(from) = last.1.filter(|_| elapsed != 0.0) else {
            return true;
        };
        let end = self.display_project().timeline.duration();
        let t = from + Time::from_secs_f64(elapsed);
        let done =
            (self.play_speed > 0.0 && t >= end) || (self.play_speed < 0.0 && t <= Time::ZERO);
        self.playhead = t.max(Time::ZERO).min(end);
        last.1 = Some(self.playhead);
        if done {
            self.playing = false;
            self.clock = None;
        }
        cx.notify();
        !done
    }
}

fn warning_text(warning: &Warning) -> String {
    match warning {
        Warning::TouchesProtected { note, .. } if note.is_empty() => {
            "Touches a protected range".to_string()
        }
        Warning::TouchesProtected { note, .. } => format!("Touches protected range “{note}”"),
        Warning::NeedsConfirmation { message } => message.clone(),
    }
}

/// Playback clock resolution.
const PLAY_TICK: Duration = Duration::from_millis(16);

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
