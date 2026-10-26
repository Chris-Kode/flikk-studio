//! Load, autosave and settings. Owned by the store work stream; backed by
//! `koo-store` under `koo_core::paths::home()`.
//!
//! One session per process: the open project, a store connection for the
//! UI thread and a background [`Autosaver`]. If the database cannot be
//! opened the editor still runs on an in-memory "Untitled" project and
//! [`take_notice`] says why.

use koo_core::agent::{AgentSettings, ChatMessage};
use koo_core::bus::CommandBus;
use koo_core::history::History;
use koo_core::ids::ProjectId;
use koo_core::model::Project;
use koo_store::{Autosaver, OpenedProject, Store};
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

/// One conversation per project (PRD AG1), as JSON beside the project's
/// other files under `paths::projects()`.
fn chat_path(project: ProjectId) -> std::path::PathBuf {
    koo_core::paths::projects().join(format!("{}.chat.json", project.0.simple()))
}

/// The project's chat. Change sets live only in memory, so messages lose
/// their review card on reload and keep their text.
pub fn load_chat(project: ProjectId) -> Vec<ChatMessage> {
    let Ok(bytes) = std::fs::read(chat_path(project)) else {
        return Vec::new();
    };
    let mut chat: Vec<ChatMessage> = serde_json::from_slice(&bytes).unwrap_or_default();
    for m in &mut chat {
        m.change_set = None;
    }
    chat
}

pub fn save_chat(project: ProjectId, chat: &[ChatMessage]) {
    let path = chat_path(project);
    let tmp = path.with_extension("json.tmp");
    let written = serde_json::to_vec(chat)
        .map_err(std::io::Error::other)
        .and_then(|bytes| std::fs::write(&tmp, bytes))
        .and_then(|()| std::fs::rename(&tmp, &path));
    if let Err(e) = written {
        notify(format!("Couldn't save the chat: {e}"));
    }
}
