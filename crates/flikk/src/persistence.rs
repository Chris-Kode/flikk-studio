//! Project files, autosave and settings. Owned by the store work stream;
//! backed by `flikk-store`.
//!
//! Projects are `.flikk` files the user saves where they like; this module
//! reads and writes them, recovers unsaved changes a crash left in
//! `paths::autosave()` and keeps the recent-files list. Whether the open
//! project has unsaved changes is the editor state's to track.
//!
//! One session per process: a store connection for the UI thread (user
//! settings, profiles, facts, recipes, recent files) and a background
//! [`Autosaver`]. If the database cannot be opened the editor still opens
//! and saves projects, and [`take_notice`] says why.

use flikk_agent::session::MAIN_CHAT;
use flikk_core::agent::{AgentSettings, Attachment, ChatMessage};
use flikk_core::bus::CommandBus;
use flikk_core::changeset::{self, ChangeSet, ItemState};
use flikk_core::ids::ProjectId;
use flikk_core::model::{Project, SequenceSettings};
use flikk_store::project_file::{self, ProjectFile};
use flikk_store::{Autosaver, Snapshot, Store};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

/// Everything needed to start editing a project.
pub struct Loaded {
    pub project: Project,
    /// The user's agent settings.
    pub settings: AgentSettings,
    /// The file it was opened from; `None` until first saved.
    pub file: Option<PathBuf>,
    /// Unsaved changes recovered from an autosave: it opens with them.
    pub recovered: bool,
}

struct Session {
    store: Option<Store>,
    autosaver: Autosaver,
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);
static NOTICE: Mutex<Option<String>> = Mutex::new(None);
/// A project file named on the command line.
static LAUNCH_FILE: OnceLock<PathBuf> = OnceLock::new();

fn session() -> MutexGuard<'static, Option<Session>> {
    SESSION.lock().unwrap_or_else(|e| e.into_inner())
}

fn notify(text: impl Into<String>) {
    *NOTICE.lock().unwrap_or_else(|e| e.into_inner()) = Some(text.into());
}

/// A one-off status line message from loading or saving (recovery, errors).
pub fn take_notice() -> Option<String> {
    NOTICE.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// Open `path` at launch instead of the most recent project.
pub fn set_launch_file(path: PathBuf) {
    let _ = LAUNCH_FILE.set(path);
}

/// The app was started to open a project file.
pub fn launched_with_file() -> bool {
    LAUNCH_FILE.get().is_some()
}

/// Start the session and pick the first project: the file named at
/// launch, else unsaved changes a crash left, else the most recent file,
/// else a new "Untitled". A demo starts on a new "Untitled" it replaces,
/// tied to none of the user's files.
pub fn load_session() -> Loaded {
    {
        let mut guard = session();
        let store = match Store::open() {
            Ok(store) => Some(store),
            Err(e) => {
                notify(format!("Settings can't be saved: {e:#}"));
                None
            }
        };
        *guard = Some(Session {
            store,
            autosaver: Autosaver::spawn(),
        });
    }
    sweep_demo_chats();
    let untitled = || new_project("Untitled", SequenceSettings::default());
    if crate::demo::enabled() {
        return untitled();
    }
    if let Some(path) = LAUNCH_FILE.get()
        && let Some(loaded) = open(path)
    {
        return loaded;
    }
    // Why the file named at launch did not open, or the store did not:
    // kept over what follows.
    let error = take_notice();
    let loaded = crash_recovery()
        .or_else(open_recent)
        .unwrap_or_else(untitled);
    if let Some(error) = error {
        notify(error);
    }
    loaded
}

/// The newest autosave a crash left that holds newer changes than its
/// file, as [`open`] would recover them. Ones whose file has caught up are
/// removed, and so are demos older versions autosaved. One whose file now
/// holds another project comes back unsaved, tied to no file.
fn crash_recovery() -> Option<Loaded> {
    for autosave in project_file::autosaves() {
        let mut file = match project_file::read(&autosave) {
            Ok(file) => file,
            Err(e) => {
                eprintln!("flikk: can't recover {}: {e:#}", autosave.display());
                continue;
            }
        };
        if crate::demo::is_seed_project(&file.project) {
            let _ = std::fs::remove_file(&autosave);
            continue;
        }
        let mut notice = "Recovered unsaved changes".to_owned();
        let fresh = match file.autosave_of.as_deref() {
            // Never saved: the autosave is the only copy.
            None => true,
            Some(path) => {
                let saved = project_file::read(path).map(|f| f.project.id);
                match saved {
                    // The file holds another project now (saved over, or
                    // replaced), so no file has these changes: they open
                    // as a project not saved yet. Saving or discarding it
                    // removes the autosave.
                    Ok(id) if id != file.project.id => {
                        notice = format!(
                            "Recovered unsaved changes; {} holds another project now",
                            project_file::name_of(path)
                        );
                        file.autosave_of = None;
                        true
                    }
                    _ if holds_newer(&autosave, &file, path) => true,
                    Ok(_) => {
                        // Only a strictly newer file has caught up: with
                        // coarse timestamps the same age may still hide
                        // changes made just after a save.
                        if modified(path) > modified(&autosave) {
                            let _ = std::fs::remove_file(&autosave);
                        }
                        false
                    }
                    // A file that is missing or broken may still need it.
                    Err(_) => false,
                }
            }
        };
        if fresh {
            notify(notice);
            return Some(loaded(file, true));
        }
    }
    None
}

/// The most recent project file that opens.
fn open_recent() -> Option<Loaded> {
    let opened = recent()
        .into_iter()
        .find_map(|path| path.exists().then(|| open(&path)).flatten())?;
    // An older error about another file is not news.
    let _ = take_notice();
    Some(opened)
}

/// A new project named `name` with a sequence of `settings`, not saved
/// anywhere yet.
pub fn new_project(name: &str, settings: SequenceSettings) -> Loaded {
    let mut project = Project::new(name);
    project.timeline.settings = settings;
    loaded(
        ProjectFile {
            project,
            autosave_of: None,
        },
        false,
    )
}

/// Read a project file, with the unsaved changes a crash left for it.
/// Errors are reported through [`take_notice`].
pub fn open(path: &Path) -> Option<Loaded> {
    let file = match project_file::read(path) {
        Ok(file) => file,
        Err(e) => {
            notify(format!("{e:#}"));
            return None;
        }
    };
    let recovered = recover(path, file.project.id);
    with_store(|s| s.add_recent_project(path));
    let mut opened = match recovered {
        Some(autosave) => {
            notify("Recovered unsaved changes");
            loaded(autosave, true)
        }
        None => loaded(file, false),
    };
    opened.file = Some(path.to_path_buf());
    opened.project.name = project_file::name_of(path);
    Some(opened)
}

/// The autosave of project `id` when it holds newer changes to `path`.
fn recover(path: &Path, id: ProjectId) -> Option<ProjectFile> {
    let autosave = project_file::autosave_path(id);
    let file = project_file::read(&autosave).ok()?;
    holds_newer(&autosave, &file, path).then_some(file)
}

/// `file`, read from `autosave`, holds changes to `path` newer than the
/// file there (or there is none).
fn holds_newer(autosave: &Path, file: &ProjectFile, path: &Path) -> bool {
    let newer = match (modified(autosave), modified(path)) {
        (Some(a), Some(f)) => a > f,
        (a, _) => a.is_some(),
    };
    let same = |p: &Path| std::fs::canonicalize(p).ok();
    newer
        && file
            .autosave_of
            .as_deref()
            .is_some_and(|of| of == path || same(of).is_some_and(|of| Some(of) == same(path)))
}

fn modified(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn loaded(file: ProjectFile, recovered: bool) -> Loaded {
    Loaded {
        project: file.project,
        settings: with_store(Store::user_settings).unwrap_or_default(),
        file: file.autosave_of,
        recovered,
    }
}

/// Write `project` to `path`: the file, a backup now and then, and the
/// recent list; its autosave is no longer needed. Errors are reported
/// through [`take_notice`].
pub fn save(path: &Path, project: &Project) -> bool {
    if let Err(e) = project_file::write(path, project) {
        notify(format!("{e:#}"));
        return false;
    }
    discard(project.id);
    if let Err(e) = project_file::backup(path, project.id) {
        eprintln!("flikk: backup of {} failed: {e:#}", path.display());
    }
    with_store(|s| s.add_recent_project(path));
    true
}

/// Queue unsaved changes for the background writer, for crash recovery.
/// A demo is not kept.
pub fn autosave(project: &Project, file: Option<&Path>) {
    if crate::demo::enabled() {
        return;
    }
    if let Some(s) = session().as_ref() {
        s.autosaver.save(Snapshot {
            project: project.clone(),
            file: file.map(Path::to_path_buf),
        });
    }
}

/// Why autosave last failed, once per run of failures: the changes stay
/// queued and are tried again meanwhile.
pub fn autosave_failure() -> Option<String> {
    session().as_ref()?.autosaver.take_failure()
}

/// The project's unsaved changes were saved or thrown away: drop its
/// autosave.
pub fn discard(project: ProjectId) {
    match session().as_ref() {
        Some(s) => s.autosaver.clear(project),
        None => project_file::remove_autosave(project),
    }
}

/// Recently opened project files, newest first.
pub fn recent() -> Vec<PathBuf> {
    with_store(|s| s.recent_projects()).unwrap_or_default()
}

pub fn forget_recent(path: &Path) {
    with_store(|s| s.remove_recent_project(path));
}

/// Save settings edited in the UI, for the user. Errors are reported
/// through [`take_notice`].
pub fn save_settings(settings: &AgentSettings) {
    with_store(|s| s.save_user_settings(settings));
}

/// App quit: pending unsaved changes are written as an autosave, so they
/// are recovered on the next start unless the user discarded them.
pub fn shutdown() {
    if let Some(s) = session().take() {
        drop(s.autosaver);
    }
}

/// Run `f` with the UI thread's store (profiles, facts, recipes, recent
/// files). `None` if the store is unavailable or `f` failed; failures are
/// reported through [`take_notice`].
pub fn with_store<T>(f: impl FnOnce(&Store) -> flikk_store::Result<T>) -> Option<T> {
    let guard = session();
    let store = guard.as_ref()?.store.as_ref()?;
    match f(store) {
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
    flikk_core::paths::projects().join(format!("{}.chats.json", project.0.simple()))
}

/// The single conversation projects had before chat sessions.
fn legacy_chat_path(project: ProjectId) -> std::path::PathBuf {
    flikk_core::paths::projects().join(format!("{}.chat.json", project.0.simple()))
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
    /// Files and folders the agent may read in this chat.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
}

impl ChatSession {
    pub fn new(id: impl Into<String>, at: i64) -> Self {
        Self {
            id: id.into(),
            title: String::new(),
            at,
            messages: Vec::new(),
            attachments: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Chats {
    pub active: String,
    pub sessions: Vec<ChatSession>,
}

/// The project's chat sessions, never empty, with the proposals still
/// waiting for review in them restored to `bus` (see [`Proposals`]). A
/// message keeps its link to a change set only if that set came back.
pub fn load_chats(bus: &mut CommandBus) -> Chats {
    let project = bus.project().id;
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
    // A deleted chat's proposals have nowhere to show.
    let sets = load_proposals(bus.project())
        .into_iter()
        .filter(|set| {
            set.chat
                .as_ref()
                .is_none_or(|c| chats.sessions.iter().any(|s| &s.id == c))
        })
        .collect();
    let restored = bus.restore_change_sets(sets);
    for m in chats.sessions.iter_mut().flat_map(|s| &mut s.messages) {
        m.change_set = m.change_set.and_then(|id| restored.get(&id).copied());
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

/// Save the project's chats. A demo's are not kept: like its project, they
/// would never be opened again.
pub fn save_chats(project: ProjectId, chats: &Chats) {
    if crate::demo::enabled() {
        return;
    }
    let written = serde_json::to_vec(chats)
        .map_err(std::io::Error::other)
        .and_then(|bytes| write_atomic(&chats_path(project), &bytes));
    match written {
        Ok(()) => {
            let _ = std::fs::remove_file(legacy_chat_path(project));
        }
        Err(e) => notify(format!("Couldn't save the chat: {e}")),
    }
}

/// Write through a temporary file, so a crash never leaves half of one.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// Format of the proposals file; one with a newer version is not read.
const PROPOSALS_VERSION: u32 = 1;

/// A project's change sets still waiting for review, beside its chats
/// under `paths::projects()`.
fn proposals_path(project: ProjectId) -> PathBuf {
    flikk_core::paths::projects().join(format!("{}.proposals.json", project.0.simple()))
}

#[derive(Serialize, Deserialize)]
struct ProposalsFile<T> {
    version: u32,
    sets: Vec<T>,
}

#[derive(Serialize, Deserialize)]
struct SavedSet {
    set: ChangeSet,
    /// [`changeset::targets_hash`] of each item, on the committed project
    /// when saved.
    targets: Vec<u64>,
}

/// Keeps the open project's proposals file in step with its change sets
/// still waiting for review, so their review cards come back when it opens
/// again. Undo history isn't saved, so neither is what was accepted.
#[derive(Default)]
pub struct Proposals {
    /// A quick summary of the bus as last looked at.
    seen: Option<u64>,
    /// What the file was last brought to: the hash of what was written,
    /// [`NO_PROPOSALS`] once removed.
    written: Option<u64>,
}

const NO_PROPOSALS: u64 = 0;

impl Proposals {
    /// Write the file if the bus's change sets changed since the last
    /// call; remove it when none wait for review. A demo's are not kept.
    pub fn sync(&mut self, bus: &CommandBus) {
        if crate::demo::enabled() {
            return;
        }
        let mut h = flikk_core::subject::Fnv::default();
        h.write(bus.project().id.0.as_bytes());
        h.write(&bus.version().to_le_bytes());
        for set in bus.change_sets() {
            h.write(set.id.0.as_bytes());
            h.write(&[set.state as u8]);
            h.write(&set.base_version.to_le_bytes());
            for item in &set.items {
                h.write(item.id.0.as_bytes());
                h.write(&[item.state as u8]);
            }
        }
        if self.seen.replace(h.0) == Some(h.0) {
            return;
        }
        let project = bus.project();
        let path = proposals_path(project.id);
        let sets = bus.waiting_change_sets();
        if sets.is_empty() {
            if self.written != Some(NO_PROPOSALS) {
                let _ = std::fs::remove_file(&path);
                self.written = Some(NO_PROPOSALS);
            }
            return;
        }
        let file = ProposalsFile {
            version: PROPOSALS_VERSION,
            sets: sets
                .into_iter()
                .map(|set| SavedSet {
                    targets: set
                        .items
                        .iter()
                        .map(|i| changeset::targets_hash(project, &i.ops))
                        .collect(),
                    set,
                })
                .collect(),
        };
        let bytes = match serde_json::to_vec(&file) {
            Ok(bytes) => bytes,
            Err(e) => {
                eprintln!("flikk: can't save proposals: {e}");
                return;
            }
        };
        let mut h = flikk_core::subject::Fnv::default();
        h.write(&bytes);
        if self.written == Some(h.0) {
            return;
        }
        match write_atomic(&path, &bytes) {
            Ok(()) => self.written = Some(h.0),
            Err(e) => notify(format!("Couldn't save the agent's proposals: {e}")),
        }
    }
}

/// The proposals saved for `project`. Items that changed something the
/// project now holds differently (it was edited elsewhere, or reopened
/// from an older save) are conflicted. Unreadable sets are skipped.
fn load_proposals(project: &Project) -> Vec<ChangeSet> {
    let path = proposals_path(project.id);
    let Ok(bytes) = std::fs::read(&path) else {
        return Vec::new();
    };
    let file = match serde_json::from_slice::<ProposalsFile<serde_json::Value>>(&bytes) {
        Ok(file) if file.version <= PROPOSALS_VERSION => file,
        Ok(file) => {
            eprintln!(
                "flikk: {} is from a newer version ({}); not read",
                path.display(),
                file.version
            );
            return Vec::new();
        }
        Err(e) => {
            eprintln!("flikk: can't read {}: {e}", path.display());
            return Vec::new();
        }
    };
    file.sets
        .into_iter()
        .filter_map(|value| match serde_json::from_value::<SavedSet>(value) {
            Ok(saved) => Some(saved),
            Err(e) => {
                eprintln!("flikk: skipped a proposal in {}: {e}", path.display());
                None
            }
        })
        .map(|SavedSet { mut set, targets }| {
            for (ix, item) in set.items.iter_mut().enumerate() {
                if item.state == ItemState::Pending
                    && targets.get(ix) != Some(&changeset::targets_hash(project, &item.ops))
                {
                    item.state = ItemState::Conflicted;
                }
            }
            set
        })
        .collect()
}

/// Remove the chats earlier demos left: each demo was a new project and its
/// chats were saved, though nothing opens it again. One is known by the
/// demo's seeded conversation, and kept when its project has an autosave
/// or a backup (it was saved to a file, which may still open it).
fn sweep_demo_chats() {
    let Ok(entries) = std::fs::read_dir(flikk_core::paths::projects()) else {
        return;
    };
    let backups: Vec<String> = std::fs::read_dir(flikk_core::paths::backups())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    for path in entries.flatten().map(|e| e.path()) {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let (id, sessions) = if let Some(id) = name.strip_suffix(".chats.json") {
            (id, true)
        } else if let Some(id) = name.strip_suffix(".chat.json") {
            (id, false)
        } else {
            continue;
        };
        let Ok(id) = uuid::Uuid::parse_str(id).map(ProjectId) else {
            continue;
        };
        // Backup folders end with the project's short id.
        let short = format!(" {id}");
        if project_file::autosave_path(id).exists() || backups.iter().any(|b| b.ends_with(&short)) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let demo = if sessions {
            serde_json::from_slice::<Chats>(&bytes).is_ok_and(|chats| {
                chats
                    .sessions
                    .iter()
                    .any(|s| crate::demo::is_seed_chat(&s.messages))
            })
        } else {
            serde_json::from_slice::<Vec<ChatMessage>>(&bytes)
                .is_ok_and(|messages| crate::demo::is_seed_chat(&messages))
        };
        if demo {
            let _ = std::fs::remove_file(&path);
        }
    }
}
