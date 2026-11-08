//! Project files from the user's side: New, Open, Save and Save As, and the
//! "Save changes?" question before anything replaces or closes a project
//! with unsaved changes. The files themselves are `persistence`'s.

use crate::state::EditorState;
use crate::ui::kit::DialogExt as _;
use flikk_core::model::SequenceSettings;
use flikk_store::project_file::{EXTENSION, sanitize};
use gpui_kit::*;
use std::path::PathBuf;
use std::rc::{Rc, Weak};

actions!(
    flikk,
    [
        NewProject,
        OpenProject,
        SaveProject,
        SaveProjectAs,
        ProjectSettings
    ]
);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-n", NewProject, None),
        KeyBinding::new("ctrl-n", NewProject, None),
        KeyBinding::new("cmd-o", OpenProject, None),
        KeyBinding::new("ctrl-o", OpenProject, None),
        KeyBinding::new("cmd-s", SaveProject, None),
        KeyBinding::new("ctrl-s", SaveProject, None),
        KeyBinding::new("cmd-shift-s", SaveProjectAs, None),
        KeyBinding::new("ctrl-shift-s", SaveProjectAs, None),
        KeyBinding::new("cmd-j", ProjectSettings, None),
        KeyBinding::new("ctrl-j", ProjectSettings, None),
    ]);
}

/// What replaces or closes the open project once its changes are settled.
#[derive(Clone)]
enum Then {
    Create(PathBuf, SequenceSettings),
    Open(PathBuf),
    Quit,
    /// Quit and start the version an update installed.
    Restart,
}

/// The "Save changes?" question or a save's file prompt, while one is up:
/// it lasts as long as something holds its token. Another waits for it to
/// end rather than ask twice or save twice.
struct Settling(Weak<()>);

impl Global for Settling {}

fn settling(cx: &App) -> bool {
    cx.try_global::<Settling>()
        .is_some_and(|s| s.0.strong_count() > 0)
}

/// Mark a question or save under way until the token is dropped.
fn begin_settling(cx: &mut App) -> Rc<()> {
    let token = Rc::new(());
    cx.set_global(Settling(Rc::downgrade(&token)));
    token
}

/// Ask for the new project's name, folder and sequence.
pub fn new_project(state: &Entity<EditorState>, window: &mut Window, cx: &mut App) {
    crate::ui::project_settings::open_new(state, None, None, window, cx);
}

/// New project with its name filled in as `name`; `on_cancel` runs when
/// the dialog closes without Create.
pub fn new_project_named(
    state: &Entity<EditorState>,
    name: Option<String>,
    on_cancel: impl FnOnce(&mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    crate::ui::project_settings::open_new(state, name, Some(Box::new(on_cancel)), window, cx);
}

/// Change the open project's sequence.
pub fn settings(state: &Entity<EditorState>, window: &mut Window, cx: &mut App) {
    crate::ui::project_settings::open_settings(state, window, cx);
}

/// Make a project saved at `path` with a sequence of `settings`, once the
/// open project's changes are settled.
pub fn create(
    state: &Entity<EditorState>,
    path: PathBuf,
    settings: SequenceSettings,
    window: &mut Window,
    cx: &mut App,
) {
    settle(state, Then::Create(path, settings), window, cx);
}

/// Pick a project file and open it.
pub fn open_dialog(state: &Entity<EditorState>, window: &mut Window, cx: &mut App) {
    let paths = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: None,
    });
    let state = state.clone();
    window
        .spawn(cx, async move |cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            if let Some(path) = paths.into_iter().next() {
                let _ = cx.update(|window, cx| open(&state, path, window, cx));
            }
        })
        .detach();
}

pub fn open(state: &Entity<EditorState>, path: PathBuf, window: &mut Window, cx: &mut App) {
    if state.read(cx).file() == Some(path.as_path()) {
        return;
    }
    settle(state, Then::Open(path), window, cx);
}

/// Save to the project's file; a project never saved asks where.
pub fn save(state: &Entity<EditorState>, window: &mut Window, cx: &mut App) {
    if settling(cx) {
        return;
    }
    let token = begin_settling(cx);
    save_then(state, None, token, window, cx);
}

/// Save to a new file, which the project continues in.
pub fn save_as(state: &Entity<EditorState>, window: &mut Window, cx: &mut App) {
    if settling(cx) {
        return;
    }
    let token = begin_settling(cx);
    let target = state.clone();
    ask_path(state, window, cx, move |path, _, cx| {
        target.update(cx, |s, cx| s.save_project(path, cx));
        drop(token);
    });
}

/// Quit, asking about unsaved changes first.
pub fn quit(state: &Entity<EditorState>, window: &mut Window, cx: &mut App) {
    settle(state, Then::Quit, window, cx);
}

/// Quit, asking about unsaved changes first, and start the version an
/// update installed.
pub fn restart(state: &Entity<EditorState>, window: &mut Window, cx: &mut App) {
    settle(state, Then::Restart, window, cx);
}

/// For the window's close button: close at once when there is nothing to
/// save, else ask and quit once settled.
pub fn should_close(state: &Entity<EditorState>, window: &mut Window, cx: &mut App) -> bool {
    if !state.read(cx).is_dirty() {
        return true;
    }
    settle(state, Then::Quit, window, cx);
    false
}

/// Save, then (when it saved) do `then`. `token` marks the save under way
/// until it ends.
fn save_then(
    state: &Entity<EditorState>,
    then: Option<Then>,
    token: Rc<()>,
    window: &mut Window,
    cx: &mut App,
) {
    if let Some(path) = state.read(cx).file().map(|f| f.to_path_buf()) {
        let saved = state.update(cx, |s, cx| s.save_project(path, cx));
        drop(token);
        if saved && let Some(then) = then {
            run(state, then, window, cx);
        }
        return;
    }
    let target = state.clone();
    ask_path(state, window, cx, move |path, window, cx| {
        let saved = target.update(cx, |s, cx| s.save_project(path, cx));
        drop(token);
        if saved && let Some(then) = then {
            run(&target, then, window, cx);
        }
    });
}

/// Where to save: beside the project's file, else in the user's videos
/// folder, named after the project.
fn ask_path(
    state: &Entity<EditorState>,
    window: &Window,
    cx: &mut App,
    done: impl FnOnce(PathBuf, &mut Window, &mut App) + 'static,
) {
    let s = state.read(cx);
    let dir = s
        .file()
        .and_then(|f| f.parent())
        .map(PathBuf::from)
        .unwrap_or_else(crate::ui::deliver::default_dir);
    let name = format!("{}.{EXTENSION}", sanitize(&s.bus.project().name));
    let rx = cx.prompt_for_new_path(&dir, Some(&name));
    let window = window.window_handle();
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(mut path))) = rx.await else {
            return;
        };
        if path.extension().is_none_or(|e| e != EXTENSION) {
            let mut name = path.file_name().unwrap_or_default().to_os_string();
            name.push(format!(".{EXTENSION}"));
            path.set_file_name(name);
        }
        let _ = window.update(cx, |_, window, cx| done(path, window, cx));
    })
    .detach();
}

/// Do `then` once unsaved changes are saved or let go, asking first. The
/// question opens over any other dialog; while it or its save is under way,
/// another ask does nothing.
fn settle(state: &Entity<EditorState>, then: Then, window: &mut Window, cx: &mut App) {
    if !state.read(cx).is_dirty() {
        run(state, then, window, cx);
        return;
    }
    if settling(cx) {
        return;
    }
    // The dialog holds the token until it closes; Save hands it on.
    let token = begin_settling(cx);
    let name = state.read(cx).bus.project().name.clone();
    let state = state.clone();
    window.open_confirm(cx, move |dialog, _, _| {
        let (save_state, discard_state) = (state.clone(), state.clone());
        let (save_then_do, discard_then_do) = (then.clone(), then.clone());
        let token = token.clone();
        dialog
            .title(format!("Save changes to “{name}”?"))
            .secondary("Don't save", move |window, cx| {
                discard_state.update(cx, |s, _| s.discard_changes());
                run(&discard_state, discard_then_do.clone(), window, cx);
                true
            })
            .confirm("Save", move |window, cx| {
                let token = token.clone();
                save_then(&save_state, Some(save_then_do.clone()), token, window, cx);
                true
            })
    });
}

fn run(state: &Entity<EditorState>, then: Then, _: &mut Window, cx: &mut App) {
    match then {
        Then::Create(path, settings) => {
            state.update(cx, |s, cx| s.create_project(path, settings, cx));
        }
        Then::Open(path) => {
            state.update(cx, |s, cx| s.open_project(&path, cx));
        }
        Then::Quit => cx.quit(),
        Then::Restart => {
            crate::update::relaunch_on_quit();
            cx.quit();
        }
    }
}
