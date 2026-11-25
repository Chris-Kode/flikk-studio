//! The projects open at once, each with its own agent.
//!
//! One project is in the editor: its state is the fields of `EditorState`
//! the panels read. The others are [`Parked`]: the same fields, set aside
//! with their agent, which goes on working. A parked project comes into the
//! editor by trading places with the one there ([`EditorState::enter_project`]),
//! and anything done for a parked project is done with it in the editor's
//! place for the length of one call ([`EditorState::in_project`]), so every
//! rule about a project's chat, approvals and edits is written once.

use super::chat::ChatLog;
use super::document::Document;
use super::offline::OfflineMedia;
use super::{ApprovalOutcome, EditorState};
use crate::agent_link::AgentLink;
use crate::persistence;
use flikk_agent::PendingApproval;
use flikk_core::agent::{ChatRole, Selection};
use flikk_core::bus::CommandBus;
use flikk_core::model::{Project, SequenceSettings};
use flikk_core::{ChangeItemId, ChangeSetId, MediaId, ProjectId, Time, TimeRange};
use gpui_kit::*;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Everything tied to one open project while another is in the editor. The
/// fields are those of `EditorState` by the same names.
pub(super) struct Parked {
    bus: CommandBus,
    document: Document,
    chat: ChatLog,
    proposals: persistence::Proposals,
    agent: AgentLink,
    unattended: bool,
    listed: bool,
    opened_at: u64,
    playhead: Time,
    in_out: Option<TimeRange>,
    selection: Selection,
    selected_transition: Option<(flikk_core::ClipId, flikk_core::command::Edge)>,
    source_marks: HashMap<MediaId, TimeRange>,
    preview: Option<ChangeSetId>,
    live: Option<Project>,
    focused_change: Option<(ChangeSetId, ChangeItemId)>,
    editing_fx: Option<flikk_core::fx::FxRef>,
    settled: Vec<(PendingApproval, ApprovalOutcome)>,
    provider_asks: Vec<MediaId>,
    media_seen: HashSet<MediaId>,
    status: Option<String>,
    offline: OfflineMedia,
}

impl Parked {
    /// A project just read from disk or made, as it is before anything is
    /// done with it: its chats and the proposals waiting in them, an agent
    /// of its own, nothing selected.
    fn load(loaded: persistence::Loaded, opened_at: u64) -> Self {
        let mut bus = CommandBus::new(loaded.project);
        bus.set_permissions(loaded.settings.permissions.clone());
        let chat = ChatLog::load(persistence::load_chats(&mut bus));
        let document = Document::opened(loaded.file, loaded.recovered, &bus);
        let media_seen = bus.project().media.iter().map(|m| m.id).collect();
        Self {
            bus,
            document,
            chat,
            proposals: persistence::Proposals::default(),
            agent: AgentLink::new(),
            unattended: false,
            listed: true,
            opened_at,
            playhead: Time::ZERO,
            in_out: None,
            selection: Selection::default(),
            selected_transition: None,
            source_marks: HashMap::new(),
            preview: None,
            live: None,
            focused_change: None,
            editing_fx: None,
            settled: Vec::new(),
            provider_asks: Vec::new(),
            media_seen,
            status: persistence::take_notice(),
            offline: OfflineMedia::default(),
        }
    }

    pub(super) fn id(&self) -> ProjectId {
        self.bus.project().id
    }

    pub(super) fn project(&self) -> &Project {
        self.bus.project()
    }

    pub(super) fn bus_mut(&mut self) -> &mut CommandBus {
        &mut self.bus
    }

    pub(super) fn agent_mut(&mut self) -> &mut AgentLink {
        &mut self.agent
    }

    /// Hand the agent the memory it works from, see
    /// [`super::agent::hand_memory`].
    pub(super) fn hand_memory(&mut self, settings: &flikk_core::agent::AgentSettings) {
        super::agent::hand_memory(settings, &mut self.bus, &self.chat, &mut self.agent);
    }

    /// Save the proposals still waiting for review beside the chats.
    pub(super) fn sync_proposals(&mut self) {
        self.proposals.sync(&self.bus);
    }

    fn unsettled(&self) -> bool {
        self.document.unsettled(&self.bus)
    }
}

/// An open project as the home page lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct OpenProject {
    pub id: ProjectId,
    pub name: String,
    pub sequence: SequenceSettings,
    pub duration: Time,
    /// It is the project in the editor.
    pub in_editor: bool,
    /// It has changes not saved to its file.
    pub edited: bool,
    pub agent: AgentState,
}

/// Where a project's agent is with its chat.
#[derive(Clone, Debug, PartialEq)]
pub enum AgentState {
    /// Nothing under way; the agent's last reply, if it wrote one.
    Idle { reply: Option<String> },
    /// A turn is under way.
    Working(Doing),
    /// The agent asked something and waits for the answer.
    Asking {
        question: String,
        choices: Vec<String>,
    },
    /// The last turn failed.
    Failed { message: String },
}

/// What a working agent was last seen doing.
#[derive(Clone, Debug, PartialEq)]
pub enum Doing {
    /// Nothing to show yet.
    Thinking,
    /// Running this tool.
    Tool(String),
    /// Writing: the line it is on.
    Writing(String),
}

/// What the home page shows of a project: `in_editor` is filled in by the
/// caller.
fn listing(
    bus: &CommandBus,
    document: &Document,
    chat: &ChatLog,
    agent: &AgentLink,
) -> OpenProject {
    let project = bus.project();
    let last = chat.messages().last();
    let line = |text: &str| {
        crate::ui::chat::plain_text(text, project)
            .lines()
            .rev()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(str::to_owned)
    };
    let state = if agent.status().is_busy() {
        AgentState::Working(match last.filter(|m| m.role == ChatRole::Agent) {
            Some(m) => match m.tools.iter().rev().find(|t| t.running.is_some()) {
                Some(tool) => Doing::Tool(tool.tool.clone()),
                None => line(&m.text).map_or(Doing::Thinking, Doing::Writing),
            },
            None => Doing::Thinking,
        })
    } else if let Some(message) = chat.agent_error() {
        AgentState::Failed {
            message: message.to_owned(),
        }
    } else if let Some(asked) = chat.open_question().and_then(|ix| chat.messages().get(ix)) {
        AgentState::Asking {
            question: crate::ui::chat::plain_text(&asked.text, project),
            choices: asked.choices.clone(),
        }
    } else {
        AgentState::Idle {
            reply: chat
                .messages()
                .iter()
                .rev()
                .filter(|m| m.role == ChatRole::Agent)
                .find_map(|m| line(&m.text)),
        }
    };
    OpenProject {
        id: project.id,
        name: project.name.clone(),
        sequence: project.timeline.settings.clone(),
        duration: project.timeline.duration(),
        in_editor: false,
        edited: document.is_dirty(bus),
        agent: state,
    }
}

impl EditorState {
    // ------------------------------------------------------------ the list

    /// The open projects the home page lists, the last opened first. The
    /// project the app started on shows once the user goes into it.
    pub fn open_projects(&self) -> Vec<OpenProject> {
        let editor = self.listed.then(|| {
            let listed = listing(&self.bus, &self.document, &self.chat, &self.agent);
            (
                self.opened_at,
                OpenProject {
                    in_editor: true,
                    ..listed
                },
            )
        });
        let parked = self
            .parked
            .iter()
            .filter(|p| p.listed)
            .map(|p| (p.opened_at, listing(&p.bus, &p.document, &p.chat, &p.agent)));
        let mut open: Vec<_> = editor.into_iter().chain(parked).collect();
        open.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
        open.into_iter().map(|(_, project)| project).collect()
    }

    /// How many open projects' agents have a turn under way.
    pub fn agents_working(&self) -> usize {
        let parked = self.parked.iter().filter(|p| p.agent.status().is_busy());
        parked.count() + usize::from(self.agent_busy())
    }

    /// Counts the projects the user brought into the editor: made, opened
    /// or entered. One that takes the place of a closed project is not
    /// counted.
    pub fn times_opened(&self) -> u64 {
        self.opened
    }

    /// The project in the editor shows on the home page from now on: the
    /// user went into the one the app started on.
    pub fn list_project(&mut self, cx: &mut Context<Self>) {
        if !self.listed {
            self.listed = true;
            cx.notify();
        }
    }

    /// Whether the project in the editor shows on the home page.
    pub fn project_listed(&self) -> bool {
        self.listed
    }

    /// The open project saved in `file`, if any.
    pub(super) fn open_in(&self, file: &Path) -> Option<ProjectId> {
        if self.document.file() == Some(file) {
            return Some(self.bus.project().id);
        }
        self.parked
            .iter()
            .find(|p| p.document.file() == Some(file))
            .map(Parked::id)
    }

    /// Whether `project` is open, in the editor or not.
    pub(super) fn is_open(&self, project: ProjectId) -> bool {
        self.bus.project().id == project || self.parked.iter().any(|p| p.id() == project)
    }

    /// The first open project with unsaved changes the user has not let
    /// go, the one in the editor before the others.
    pub fn unsettled_project(&self) -> Option<ProjectId> {
        if self.document.unsettled(&self.bus) {
            return Some(self.bus.project().id);
        }
        self.parked.iter().find(|p| p.unsettled()).map(Parked::id)
    }

    /// Whether open project `project` has unsaved changes the user has not
    /// let go.
    pub fn is_unsettled(&self, project: ProjectId) -> bool {
        if self.bus.project().id == project {
            return self.document.unsettled(&self.bus);
        }
        self.parked
            .iter()
            .any(|p| p.id() == project && p.unsettled())
    }

    // ------------------------------------------------------ trading places

    /// Trade every per-project field with `other`.
    fn trade(&mut self, other: &mut Parked) {
        use std::mem::swap;
        swap(&mut self.bus, &mut other.bus);
        swap(&mut self.document, &mut other.document);
        swap(&mut self.chat, &mut other.chat);
        swap(&mut self.proposals, &mut other.proposals);
        swap(&mut self.agent, &mut other.agent);
        swap(&mut self.unattended, &mut other.unattended);
        swap(&mut self.listed, &mut other.listed);
        swap(&mut self.opened_at, &mut other.opened_at);
        swap(&mut self.playhead, &mut other.playhead);
        swap(&mut self.in_out, &mut other.in_out);
        swap(&mut self.selection, &mut other.selection);
        swap(
            &mut self.selected_transition,
            &mut other.selected_transition,
        );
        swap(&mut self.source_marks, &mut other.source_marks);
        swap(&mut self.preview, &mut other.preview);
        swap(&mut self.live, &mut other.live);
        swap(&mut self.focused_change, &mut other.focused_change);
        swap(&mut self.editing_fx, &mut other.editing_fx);
        swap(&mut self.settled, &mut other.settled);
        swap(&mut self.provider_asks, &mut other.provider_asks);
        swap(&mut self.media_seen, &mut other.media_seen);
        swap(&mut self.status, &mut other.status);
        swap(&mut self.offline, &mut other.offline);
    }

    /// Run `f` with open project `project` in the editor's place; `None`
    /// when it is not open. The panels never see the trade: it lasts for
    /// this call. `f` must not open, enter or close a project.
    pub(super) fn in_project<R>(
        &mut self,
        project: ProjectId,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Self, &mut Context<Self>) -> R,
    ) -> Option<R> {
        if self.bus.project().id == project {
            return Some(f(self, cx));
        }
        let ix = self.parked.iter().position(|p| p.id() == project)?;
        Some(self.visit(ix, cx, f))
    }

    /// Run `f` for `project` when it is open, else with the project in the
    /// editor, for work that tells by itself whose it was and keeps what
    /// it made for a closed project.
    pub(super) fn in_project_or_here<R>(
        &mut self,
        project: ProjectId,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Self, &mut Context<Self>) -> R,
    ) -> R {
        match self.parked.iter().position(|p| p.id() == project) {
            Some(ix) if !self.visiting => self.visit(ix, cx, f),
            _ => f(self, cx),
        }
    }

    /// Run `f` with parked project `ix` in the editor's place. Meanwhile
    /// the editor's own project is the last of `parked`, so what goes over
    /// every open project still reaches it.
    fn visit<R>(
        &mut self,
        ix: usize,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Self, &mut Context<Self>) -> R,
    ) -> R {
        debug_assert!(!self.visiting, "a visit inside a visit");
        let last = self.parked.len() - 1;
        let mut other = self.parked.swap_remove(ix);
        self.trade(&mut other);
        self.parked.push(other);
        self.visiting = true;
        let out = f(self, cx);
        self.visiting = false;
        debug_assert_eq!(
            self.parked.len(),
            last + 1,
            "a visit changed the open projects"
        );
        let mut visited = self.parked.pop().expect("pushed above");
        self.trade(&mut visited);
        self.parked.push(visited);
        self.parked.swap(ix, last);
        out
    }

    /// Serve every open project's agent: its tool calls against its own
    /// project, and what its turn brings into its own chat.
    pub(super) fn pump_agents(&mut self, cx: &mut Context<Self>) {
        self.pump_agent(cx);
        for ix in 0..self.parked.len() {
            self.visit(ix, cx, |this, cx| this.pump_agent(cx));
        }
    }

    // ------------------------------------------------- opening and leaving

    /// Bring `loaded` into the editor. The project there stays open,
    /// parked; its engines stay, and undo history does not carry over.
    pub(super) fn switch_to(&mut self, loaded: persistence::Loaded, cx: &mut Context<Self>) {
        self.settings = loaded.settings.clone();
        self.next_opened += 1;
        let incoming = Parked::load(loaded, self.next_opened);
        self.bring_in(incoming, cx);
        self.opened += 1;
        self.load_agent_memory();
    }

    /// Go into open project `project`: it takes the editor, and the one
    /// there is parked. The project already there shows on the home page
    /// from now on.
    pub fn enter_project(&mut self, project: ProjectId, cx: &mut Context<Self>) {
        if self.front_project(project, cx) {
            self.listed = true;
            self.opened += 1;
            cx.notify();
        }
    }

    /// Put open project `project` in the editor without counting it as
    /// opened, for a question that is about it (saving it before it
    /// closes). Whether it is there now.
    pub fn front_project(&mut self, project: ProjectId, cx: &mut Context<Self>) -> bool {
        if self.bus.project().id == project {
            return true;
        }
        let Some(ix) = self.parked.iter().position(|p| p.id() == project) else {
            return false;
        };
        let incoming = self.parked.remove(ix);
        self.bring_in(incoming, cx);
        true
    }

    /// Put `incoming` in the editor and park the project that was there.
    /// The one the app started on, never gone into and unchanged, closes
    /// instead: nobody asked for it.
    fn bring_in(&mut self, mut incoming: Parked, cx: &mut Context<Self>) {
        self.leave_editor(cx);
        self.trade(&mut incoming);
        let mut left = incoming;
        if left.listed || left.document.is_dirty(&left.bus) {
            left.listed = true;
            self.parked.push(left);
        } else {
            self.close(left, cx);
        }
        self.enter_editor(cx);
    }

    /// The project in the editor is about to leave it: what only the
    /// editor does for a project stops.
    fn leave_editor(&mut self, cx: &mut Context<Self>) {
        self.set_playing(false, cx);
        // The pool item in the viewer plays on the editor's clock.
        self.source = None;
        self.clear_live();
        self.save_chat();
        self.proposals.sync(&self.bus);
        self.agent.unpublish();
    }

    /// A project just took the editor: reset what the editor holds for
    /// whichever project is in it.
    fn enter_editor(&mut self, cx: &mut Context<Self>) {
        self.agent.publish();
        self.pending_mentions.clear();
        self.depth_pick = None;
        // The notice is the editor's: it now lists this project's media.
        self.offline = OfflineMedia::default();
        self.check_offline();
        self.load_presets();
        self.add_kept(cx);
        cx.notify();
    }

    /// Close open project `project`: its agent stops, and its chat and
    /// proposals are saved. Unsaved changes must be settled first (see
    /// `crate::projects`). When it is the one in the editor, the last
    /// opened of the others takes its place, else a new "Untitled".
    pub fn close_project(&mut self, project: ProjectId, cx: &mut Context<Self>) {
        if self.bus.project().id != project {
            if let Some(ix) = self.parked.iter().position(|p| p.id() == project) {
                let closed = self.parked.remove(ix);
                self.close(closed, cx);
                cx.notify();
            }
            return;
        }
        let next = (0..self.parked.len()).max_by_key(|&ix| self.parked[ix].opened_at);
        let incoming = match next {
            Some(ix) => self.parked.remove(ix),
            None => {
                let untitled = persistence::new_project("Untitled", SequenceSettings::default());
                self.next_opened += 1;
                let mut untitled = Parked::load(untitled, self.next_opened);
                untitled.listed = false;
                untitled
            }
        };
        self.leave_editor(cx);
        let mut closed = incoming;
        self.trade(&mut closed);
        self.close(closed, cx);
        self.enter_editor(cx);
        self.load_agent_memory();
    }

    /// `closed` is open no longer: stop its agent, with what its turn got
    /// to saved in its chat, and the background work on media no open
    /// project has.
    fn close(&mut self, mut closed: Parked, cx: &mut Context<Self>) {
        let media = closed.project().media.clone();
        self.parked.push(closed);
        let last = self.parked.len() - 1;
        self.visit(last, cx, |this, cx| {
            this.agent.cancel();
            // Nothing more is approved for it: what its turn proposed and
            // asked for stays as it is, saved with its chat.
            this.unattended = false;
            // The stopped turn ends on this pump.
            this.pump_agent(cx);
            this.save_chat();
            this.proposals.sync(&this.bus);
        });
        closed = self.parked.pop().expect("pushed above");
        drop(closed);
        self.cancel_left(&media);
    }

    /// Every parked project, for what goes over all the open ones.
    pub(super) fn parked_mut(&mut self) -> impl Iterator<Item = &mut Parked> {
        self.parked.iter_mut()
    }

    /// Every parked project.
    pub(super) fn parked(&self) -> impl Iterator<Item = &Parked> {
        self.parked.iter()
    }

    /// Bring `media` into open project `project`'s pool as the user's
    /// edit, in the editor or not. Whether it went in.
    pub fn import_media(
        &mut self,
        project: ProjectId,
        media: flikk_core::model::MediaItem,
        cx: &mut Context<Self>,
    ) -> bool {
        let import = flikk_core::command::Command::ImportMedia { media };
        self.in_project(project, cx, |this, cx| this.execute(import, cx).is_some())
            .unwrap_or(false)
    }

    // -------------------------------------------------- agents from home

    /// Send `text` to open project `project`'s agent as the user's message
    /// from the home page. Its turn runs unattended: what it asks approval
    /// for is approved as it asks, and its edits are applied when it ends.
    /// Nothing while that agent is busy.
    pub fn send_unattended(&mut self, project: ProjectId, text: String, cx: &mut Context<Self>) {
        self.in_project(project, cx, |this, cx| {
            if this.agent_busy() {
                return;
            }
            this.send_chat(text, cx);
            this.unattended = true;
        });
    }

    /// Stop the turn open project `project`'s agent is on.
    pub fn stop_agent(&mut self, project: ProjectId, cx: &mut Context<Self>) {
        self.in_project(project, cx, |this, cx| this.cancel_agent(cx));
    }
}
