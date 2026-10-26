//! Load, autosave and settings. Owned by the store work stream; backed by
//! `koo-store` under `koo_core::paths::home()`.
//!
//! One session per process: the open project, a store connection for the
//! UI thread and a background [`Autosaver`]. If the database cannot be
//! opened the editor still runs on an in-memory "Untitled" project and
//! [`take_notice`] says why.

use koo_agent::session::MAIN_CHAT;
use koo_core::agent::{AgentSettings, ChatMessage};
use koo_core::bus::CommandBus;
use koo_core::history::History;
use koo_core::ids::ProjectId;
use koo_core::model::Project;
use koo_store::{Autosaver, OpenedProject, Store};
use serde::{Deserialize, Serialize};
use std::sync::{Mutex, MutexGuard};

/// Everything needed to start editing a project.
pub struct Loaded {
    pub project: Project,
    pub history: History,
    pub settings: AgentSettings,
}

struct Session {
    store: Store,
    autosaver: Autosaver,
    project: ProjectId,
    /// Bus version of the last snapshot handed to the autosaver.
    saved_version: Option<u64>,
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);
static NOTICE: Mutex<Option<String>> = Mutex::new(None);

fn session() -> MutexGuard<'static, Option<Session>> {
    SESSION.lock().unwrap_or_else(|e| e.into_inner())
}

fn notify(text: impl Into<String>) {
    *NOTICE.lock().unwrap_or_else(|e| e.into_inner()) = Some(text.into());
}

/// A one-off status line message from loading (recovery, errors).
pub fn take_notice() -> Option<String> {
    NOTICE.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// Open the last project (recovering a crashed session's autosave), or
/// create "Untitled". Also starts the autosaver.
pub fn load_session() -> Loaded {
    match try_load() {
        Ok(loaded) => loaded,
        Err(e) => {
            notify(format!("Projects can't be saved: {e:#}"));
            Loaded {
                project: Project::new("Untitled"),
                history: History::default(),
                settings: AgentSettings::default(),
            }
        }
    }
}

fn try_load() -> koo_store::Result<Loaded> {
    let mut guard = session();
    if let Some(old) = guard.take() {
        old.autosaver.close();
    }
    let store = Store::open()?;
    let autosaver = Autosaver::spawn(store.reopen()?);
    let opened = store.open_last_or_new()?;
    let loaded = loaded(&store, opened)?;
    *guard = Some(Session {
        store,
        autosaver,
        project: loaded.project.id,
        saved_version: None,
    });
    Ok(loaded)
}

fn loaded(store: &Store, opened: OpenedProject) -> koo_store::Result<Loaded> {
    if opened.recovered {
        notify("Recovered unsaved changes");
    }
    Ok(Loaded {
        settings: store.load_settings(opened.project.id)?,
        project: opened.project,
        history: opened.history,
    })
}

/// Queue a snapshot of the bus for the background writer. Cheap: skips
/// unchanged versions, and the clone is the only work on this thread.
pub fn autosave(bus: &CommandBus) {
    let mut guard = session();
    let Some(s) = guard.as_mut() else {
        return;
    };
    let project = bus.project();
    if project.id == s.project && s.saved_version == Some(bus.version()) {
        return;
    }
    s.project = project.id;
    s.saved_version = Some(bus.version());
    s.autosaver.save(project.clone(), bus.history().clone());
}

/// Save settings edited in the UI. Fields the open project overrides stay
/// project-specific; everything else is saved for the user.
pub fn save_settings(settings: &AgentSettings) {
    with_session(|s| s.store.save_effective_settings(s.project, settings));
}

/// Switch projects: the current one is closed cleanly first.
pub fn open_project(id: ProjectId) -> Option<Loaded> {
    with_session(|s| {
        s.autosaver.close();
        let Some(opened) = s.store.open_project(id)? else {
            anyhow::bail!("project not found");
        };
        switch(s, opened)
    })
}

/// Create a project and switch to it.
pub fn new_project(name: &str) -> Option<Loaded> {
    with_session(|s| {
        s.autosaver.close();
        let project = s.store.create_project(name)?;
        s.store.set_last_project(Some(project.id))?;
        switch(
            s,
            OpenedProject {
                project,
                history: History::default(),
                recovered: false,
            },
        )
    })
}

fn switch(s: &mut Session, opened: OpenedProject) -> koo_store::Result<Loaded> {
    let loaded = loaded(&s.store, opened)?;
    s.project = loaded.project.id;
    s.saved_version = None;
    Ok(loaded)
}

/// Clean exit: write and close the open project. Call on app quit; without
/// it the next start treats the session as crashed and recovers it.
pub fn shutdown() {
    if let Some(s) = session().take() {
        drop(s.autosaver);
    }
}

/// Run `f` with the UI thread's store (project list, profiles, facts,
/// recipes). `None` if the store is unavailable or `f` failed; failures are
/// reported through [`take_notice`].
pub fn with_store<T>(f: impl FnOnce(&Store) -> koo_store::Result<T>) -> Option<T> {
    with_session(|s| f(&s.store))
}

fn with_session<T>(f: impl FnOnce(&mut Session) -> koo_store::Result<T>) -> Option<T> {
    let mut guard = session();
    let s = guard.as_mut()?;
    match f(s) {
        Ok(v) => Some(v),
        Err(e) => {
            notify(format!("{e:#}"));
            None
        }
    }
}

/// A project's chat sessions, as JSON beside the project's other files
/// under `paths::projects()`.
fn chats_path(project: ProjectId) -> std::path::PathBuf {
    koo_core::paths::projects().join(format!("{}.chats.json", project.0.simple()))
}

/// The single conversation projects had before chat sessions.
fn legacy_chat_path(project: ProjectId) -> std::path::PathBuf {
    koo_core::paths::projects().join(format!("{}.chat.json", project.0.simple()))
}

/// One conversation with the agent. Each resumes its own backend session,
/// so a new chat starts without the context of the others.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChatSession {
    pub id: String,
    pub title: String,
    /// Unix seconds of the last message, or of creation.
    pub at: i64,
    pub messages: Vec<ChatMessage>,
}

impl ChatSession {
    pub fn new(id: impl Into<String>, at: i64) -> Self {
        Self {
            id: id.into(),
            title: String::new(),
            at,
            messages: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Chats {
    pub active: String,
    pub sessions: Vec<ChatSession>,
}

/// The project's chat sessions; never empty. Change sets live only in
/// memory, so messages lose their review card on reload and keep their text.
pub fn load_chats(project: ProjectId) -> Chats {
    let mut chats = std::fs::read(chats_path(project))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Chats>(&bytes).ok())
        .unwrap_or_else(|| legacy_chats(project));
    if chats.sessions.is_empty() {
        chats.sessions.push(ChatSession::new(MAIN_CHAT, 0));
    }
    if !chats.sessions.iter().any(|s| s.id == chats.active) {
        chats.active = chats.sessions[0].id.clone();
    }
    for m in chats.sessions.iter_mut().flat_map(|s| &mut s.messages) {
        m.change_set = None;
    }
    chats
}

/// The pre-sessions chat becomes the main session, which keeps resuming
/// the same backend conversation.
fn legacy_chats(project: ProjectId) -> Chats {
    let messages: Vec<ChatMessage> = std::fs::read(legacy_chat_path(project))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let mut main = ChatSession::new(MAIN_CHAT, messages.last().map_or(0, |m| m.at));
    main.messages = messages;
    Chats {
        active: main.id.clone(),
        sessions: vec![main],
    }
}

pub fn save_chats(project: ProjectId, chats: &Chats) {
    let path = chats_path(project);
    let tmp = path.with_extension("json.tmp");
    let written = serde_json::to_vec(chats)
        .map_err(std::io::Error::other)
        .and_then(|bytes| std::fs::write(&tmp, bytes))
        .and_then(|()| std::fs::rename(&tmp, &path));
    match written {
        Ok(()) => {
            let _ = std::fs::remove_file(legacy_chat_path(project));
        }
        Err(e) => notify(format!("Couldn't save the chat: {e}")),
    }
}
