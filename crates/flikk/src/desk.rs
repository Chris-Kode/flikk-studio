//! The open projects. Each is a [`Session`]: an editor state of its own,
//! with its own agent, and the panels that show it. One is in the editor;
//! the home page shows them all side by side, each agent's chat and
//! picture live, so several videos are worked on at once.
//!
//! Projects are opened, made and closed here (`crate::projects` asks about
//! unsaved changes first). There is always a session in the editor: the one
//! the app started on, and a new "Untitled" once the last project closes,
//! neither of which shows on the home page until the user goes into it.
//!
//! Other apps work here too, over MCP ([`clients`]): they start projects
//! with an agent on a message, and their calls go to the open project
//! each names. When something here waits for the user while they are in
//! another app, a system notification tells them ([`attention`]).

mod attention;
mod clients;

pub(crate) use attention::waits_in;

use crate::persistence;
use crate::state::{EditorState, SharedChange};
use crate::ui::caption_style::{CaptionStylePanel, Target};
use crate::ui::{
    chat::ChatPanel, code_editor::CodeEditorPanel, deliver::DeliverPanel,
    inspector::InspectorPanel, library::LibraryPanel, media_pool::MediaPoolPanel,
    mixer::MixerPanel, scopes::ScopesPanel, timeline::TimelinePanel, transcript::TranscriptPanel,
    viewer::ViewerPanel,
};
use flikk_core::ProjectId;
use flikk_core::model::SequenceSettings;
use gpui_kit::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// One open project: its state and the panels that show it, in the editor
/// and on the home page alike.
#[derive(Clone)]
pub struct Session {
    pub state: Entity<EditorState>,
    pub timeline: Entity<TimelinePanel>,
    pub viewer: Entity<ViewerPanel>,
    pub media_pool: Entity<MediaPoolPanel>,
    pub inspector: Entity<InspectorPanel>,
    pub transcript: Entity<TranscriptPanel>,
    pub caption_style: Entity<CaptionStylePanel>,
    pub deliver: Entity<DeliverPanel>,
    pub chat: Entity<ChatPanel>,
    pub scopes: Entity<ScopesPanel>,
    pub mixer: Entity<MixerPanel>,
    pub library: Entity<LibraryPanel>,
    pub code_editor: Entity<CodeEditorPanel>,
    _shared: std::rc::Rc<Subscription>,
}

impl Session {
    fn new(state: Entity<EditorState>, window: &mut Window, cx: &mut Context<Desk>) -> Self {
        let shared = Desk::share(&state, cx);
        Self {
            timeline: cx.new(|cx| TimelinePanel::new(state.clone(), window, cx)),
            viewer: cx.new(|cx| ViewerPanel::new(state.clone(), window, cx)),
            media_pool: cx.new(|cx| MediaPoolPanel::new(state.clone(), window, cx)),
            inspector: cx.new(|cx| InspectorPanel::new(state.clone(), window, cx)),
            transcript: cx.new(|cx| TranscriptPanel::new(state.clone(), window, cx)),
            caption_style: cx
                .new(|cx| CaptionStylePanel::new(state.clone(), Target::Active, window, cx)),
            deliver: cx.new(|cx| DeliverPanel::new(state.clone(), window, cx)),
            chat: cx.new(|cx| ChatPanel::new(state.clone(), window, cx)),
            scopes: cx.new(|cx| ScopesPanel::new(state.clone(), window, cx)),
            mixer: cx.new(|cx| MixerPanel::new(state.clone(), window, cx)),
            library: cx.new(|cx| LibraryPanel::new(state.clone(), window, cx)),
            code_editor: cx.new(|cx| CodeEditorPanel::new(state.clone(), window, cx)),
            state,
            _shared: std::rc::Rc::new(shared),
        }
    }

    pub fn project(&self, cx: &App) -> ProjectId {
        self.state.read(cx).bus.project().id
    }
}

pub struct Desk {
    /// The open projects, in the order they were opened. Never empty.
    sessions: Vec<Session>,
    /// The one in the editor.
    editor: Entity<EditorState>,
    /// See [`Self::draft`].
    draft: Option<(Entity<EditorState>, Subscription)>,
    /// See [`Self::times_opened`].
    opened: u64,
    /// What other apps ask over MCP.
    clients: clients::Clients,
    /// What the user was told waits for them.
    attention: attention::Attention,
}

struct TheDesk(Entity<Desk>);

impl Global for TheDesk {}

impl Desk {
    /// The desk of the app's one window, with the project the session
    /// starts on in the editor.
    pub fn start(window: &mut Window, cx: &mut App) -> Entity<Self> {
        let desk = cx.new(|cx| {
            let state = cx.new(EditorState::new);
            if crate::demo::enabled() {
                state.update(cx, crate::demo::seed);
            }
            state.update(cx, |s, cx| s.enter_editor(cx));
            Self {
                sessions: vec![Session::new(state.clone(), window, cx)],
                editor: state,
                draft: None,
                opened: 0,
                clients: clients::Clients::open(window, cx),
                attention: attention::Attention::start(window, cx),
            }
        });
        cx.set_global(TheDesk(desk.clone()));
        desk
    }

    /// The app's desk.
    pub fn get(cx: &App) -> Entity<Self> {
        cx.global::<TheDesk>().0.clone()
    }

    /// What `state` saves for every project (settings, the library,
    /// presets, keys), the other states read again.
    fn share(state: &Entity<EditorState>, cx: &mut Context<Self>) -> Subscription {
        cx.subscribe(state, |desk, from, change: &SharedChange, cx| {
            let draft = desk.draft.iter().map(|(draft, _)| draft);
            let others: Vec<_> = (desk.states().chain(draft))
                .filter(|state| **state != from)
                .cloned()
                .collect();
            for state in others {
                state.update(cx, |s, cx| s.read_again(*change, cx));
            }
        })
    }

    /// The session being written on the home page, before its project
    /// exists: a state of its own, so the message that starts an agent is
    /// written with everything a chat's composer has (style, skills, model
    /// and thinking level, attachments). Not one of the open projects.
    pub fn draft(&mut self, cx: &mut Context<Self>) -> Entity<EditorState> {
        if let Some((draft, _)) = &self.draft {
            return draft.clone();
        }
        let untitled = persistence::new_project("Untitled", SequenceSettings::default());
        let shared = self.editor.read(cx).shared();
        let draft = cx.new(|cx| EditorState::open(untitled, shared, cx));
        self.draft = Some((draft.clone(), Self::share(&draft, cx)));
        draft
    }

    /// The draft's message started a project: forget the draft, with what
    /// was attached to it. The next one starts empty.
    pub fn forget_draft(&mut self, cx: &mut Context<Self>) {
        if let Some((draft, _)) = self.draft.take() {
            persistence::forget_chats(draft.read(cx).bus.project().id);
        }
    }

    /// The session in the editor.
    pub fn editor(&self) -> &Session {
        self.session(&self.editor)
            .expect("the editor's session is open")
    }

    /// The session of `state`, while its project is open.
    pub fn session(&self, state: &Entity<EditorState>) -> Option<&Session> {
        self.sessions.iter().find(|s| &s.state == state)
    }

    /// The open projects the home page shows, the last opened first. The
    /// one the app started on shows once the user goes into it.
    pub fn listed(&self, cx: &App) -> Vec<Session> {
        let listed = self.sessions.iter().rev();
        listed
            .filter(|s| s.state.read(cx).is_listed())
            .cloned()
            .collect()
    }

    /// Every open project's state.
    pub fn states(&self) -> impl Iterator<Item = &Entity<EditorState>> {
        self.sessions.iter().map(|s| &s.state)
    }

    /// How many open projects' agents have a turn under way.
    pub fn agents_working(&self, cx: &App) -> usize {
        let busy = self.states().filter(|s| s.read(cx).agent_busy());
        busy.count()
    }

    /// Counts the projects the user brought into the editor: made, opened
    /// or entered. One that takes the place of a closed project is not
    /// counted.
    pub fn times_opened(&self) -> u64 {
        self.opened
    }

    /// The first open project with unsaved changes the user has not let
    /// go, the one in the editor before the others.
    pub fn unsettled(&self, cx: &App) -> Option<Entity<EditorState>> {
        let editor = std::iter::once(&self.editor);
        let mut states = editor.chain(self.states());
        states.find(|s| s.read(cx).is_unsettled()).cloned()
    }

    /// Make a project saved at `path`, which names it, with a sequence of
    /// `settings`, and bring it into the editor. The projects open stay
    /// open. Whether it saved.
    pub fn create(
        &mut self,
        path: PathBuf,
        settings: SequenceSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let name = flikk_store::project_file::name_of(&path);
        let state = self.add(persistence::new_project(&name, settings), window, cx);
        self.enter(&state, cx);
        state.update(cx, |s, cx| s.save_project(path, cx))
    }

    /// Bring the project in a file into the editor, the projects open
    /// staying open: read from the file, or the open project that is saved
    /// there. Why it did not open shows on the status line of the project
    /// in the editor. Whether it is in the editor now.
    pub fn open(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let saved_here = |s: &&Session| s.state.read(cx).file() == Some(path);
        if let Some(open) = self.sessions.iter().find(saved_here).cloned() {
            self.enter(&open.state, cx);
            return true;
        }
        let Some(loaded) = persistence::open(path) else {
            if !path.exists() {
                persistence::forget_recent(path);
            }
            let notice = persistence::take_notice();
            self.editor.update(cx, |s, cx| {
                s.status = notice;
                cx.notify();
            });
            return false;
        };
        // A copy of an open project's file: they are one project, and only
        // one of them can be open.
        let id = loaded.project.id;
        let copy = self.sessions.iter().find(|s| s.project(cx) == id).cloned();
        if let Some(open) = copy {
            self.enter(&open.state, cx);
            open.state.update(cx, |s, cx| {
                s.say("This project is already open, from another file", cx)
            });
            return true;
        }
        let state = self.add(loaded, window, cx);
        self.enter(&state, cx);
        true
    }

    /// Open a session for `loaded`, not in the editor yet.
    fn add(
        &mut self,
        loaded: persistence::Loaded,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<EditorState> {
        let shared = self.editor.read(cx).shared();
        let state = cx.new(|cx| EditorState::open(loaded, shared, cx));
        self.sessions.push(Session::new(state.clone(), window, cx));
        state
    }

    /// Put the agent of open project `state` to work on `prompt`, once
    /// `footage` is attached to its chat and in its pool: in `style` (a
    /// style's id) when one is given, else in the style selected.
    pub fn brief(
        &mut self,
        state: &Entity<EditorState>,
        prompt: String,
        footage: Vec<PathBuf>,
        style: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.session(state).cloned() else {
            return;
        };
        // What is attached to the message the agent may read, and the
        // media in it is the project's footage.
        state.update(cx, |s, cx| {
            let (project, chat) = (s.bus.project().id, s.active_chat().to_owned());
            s.attach(project, &chat, footage.clone(), cx);
        });
        let imported = (!footage.is_empty()).then(|| {
            let pool = &session.media_pool;
            pool.update(cx, |pool, cx| pool.importing(footage, None, cx))
        });
        self.clients.briefing(state);
        let state = state.clone();
        cx.spawn(async move |desk, cx| {
            if let Some(imported) = imported {
                imported.await;
            }
            let _ = desk.update(cx, |desk, cx| {
                desk.clients.briefed(&state);
                // Closed while its footage came in: its agent stays stopped.
                if desk.session(&state).is_none() {
                    return;
                }
                state.update(cx, |s, cx| match style {
                    Some(style) => s.send_chat_in_style(prompt, style, cx),
                    None => s.send_chat(prompt, cx),
                });
            });
        })
        .detach();
    }

    /// Go into open project `state`: it takes the editor, and shows on the
    /// home page from now on.
    pub fn enter(&mut self, state: &Entity<EditorState>, cx: &mut Context<Self>) {
        if self.session(state).is_none() {
            return;
        }
        self.put_in_editor(state, cx);
        state.update(cx, |s, cx| s.list(cx));
        self.opened += 1;
        cx.notify();
    }

    /// Put `state` in the editor in place of the project there, which
    /// stays open. The one the app started on, never gone into and
    /// unchanged, closes instead: nobody asked for it.
    fn put_in_editor(&mut self, state: &Entity<EditorState>, cx: &mut Context<Self>) {
        if &self.editor == state {
            return;
        }
        let left = std::mem::replace(&mut self.editor, state.clone());
        left.update(cx, |s, cx| s.leave_editor(cx));
        state.update(cx, |s, cx| s.enter_editor(cx));
        let unasked = {
            let s = left.read(cx);
            !s.is_listed() && !s.is_dirty()
        };
        if unasked {
            self.drop_session(&left, cx);
        }
    }

    /// Close open project `state`: its agent stops, and its chat and
    /// proposals are saved. Unsaved changes must be settled first (see
    /// `crate::projects`). When it is the one in the editor, the last
    /// opened of the others takes its place, else a new "Untitled".
    pub fn close(
        &mut self,
        state: &Entity<EditorState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.session(state).is_none() {
            return;
        }
        if &self.editor == state {
            let next = (self.sessions.iter().rev())
                .find(|s| &s.state != state)
                .map(|s| s.state.clone());
            let next = next.unwrap_or_else(|| {
                let untitled = persistence::new_project("Untitled", SequenceSettings::default());
                let untitled = self.add(untitled, window, cx);
                untitled.update(cx, |s, _| s.unlist());
                untitled
            });
            self.editor = next.clone();
            next.update(cx, |s, cx| s.enter_editor(cx));
        }
        self.drop_session(state, cx);
        cx.notify();
    }

    /// Take `state` out of the open projects and shut it down.
    fn drop_session(&mut self, state: &Entity<EditorState>, cx: &mut Context<Self>) {
        self.sessions.retain(|s| &s.state != state);
        let elsewhere: HashSet<flikk_core::model::Fingerprint> = (self.states())
            .flat_map(|s| s.read(cx).fingerprints().collect::<Vec<_>>())
            .collect();
        state.update(cx, |s, cx| s.shut_down(&elsewhere, cx));
    }
}
