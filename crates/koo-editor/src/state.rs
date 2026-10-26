//! Shared editor state. One entity per window; panels observe it and call
//! its methods instead of touching the bus directly, so every change goes
//! through the same path whether it came from a button or the agent.

use crate::agent_link::AgentLink;
use crate::persistence;
use gpui_kit::*;
use koo_core::agent::*;
use koo_core::bus::{AcceptReport, Author, CommandBus, Outcome};
use koo_core::command::Command;
use koo_core::model::Project;
use koo_core::services::Services;
use koo_core::{ChangeItemId, ChangeSetId, Time, TimeRange};
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
}

impl EditorState {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let (project, settings) = persistence::load_or_new();
        let mut bus = CommandBus::new(project);
        bus.set_permissions(settings.permissions.clone());
        let pump = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(AGENT_PUMP).await;
                if this.update(cx, |this, cx| this.pump_agent(cx)).is_err() {
                    break;
                }
            }
        });
        Self {
            bus,
            services: Services::default(),
            workspace: Workspace::default(),
            playhead: Time::ZERO,
            in_out: None,
            selection: Selection::default(),
            playing: false,
            settings,
            chat: Vec::new(),
            activity: Vec::new(),
            preview: None,
            status: None,
            agent: AgentLink::new(),
            _pump: pump,
        }
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

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
