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

use flikk_core::agent::AgentSettings;
use flikk_core::ids::ProjectId;
use flikk_core::model::{Project, SequenceSettings};
use flikk_store::project_file::{self, ProjectFile};
use flikk_store::{Autosaver, Snapshot, Store};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

mod chats;
mod proposals;

pub use chats::{ChatSession, Chats, load_chats, save_chats};
pub use proposals::Proposals;

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
    flikk_os::sync::lock(&SESSION)
}

fn notify(text: impl Into<String>) {
    *flikk_os::sync::lock(&NOTICE) = Some(text.into());
}

/// A one-off status line message from loading or saving (recovery, errors).
pub fn take_notice() -> Option<String> {
    flikk_os::sync::lock(&NOTICE).take()
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
    chats::sweep_demo_chats();
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
    match call_store(f)? {
        Ok(v) => Some(v),
        Err(e) => {
            notify(e);
            None
        }
    }
}

/// Run `f` with the UI thread's store, for a caller that reports the
/// error itself: the text of `f`'s error, or that the store is
/// unavailable. Nothing goes through [`take_notice`], so the error is this
/// call's own and no other notice is taken or left behind.
pub fn try_store<T>(f: impl FnOnce(&Store) -> flikk_store::Result<T>) -> Result<T, String> {
    call_store(f).unwrap_or_else(|| Err("The store is unavailable".into()))
}

/// `f`'s result with the store, its error as text; `None` when the store
/// is unavailable.
fn call_store<T>(f: impl FnOnce(&Store) -> flikk_store::Result<T>) -> Option<Result<T, String>> {
    let guard = session();
    let store = guard.as_ref()?.store.as_ref()?;
    Some(f(store).map_err(|e| format!("{e:#}")))
}
