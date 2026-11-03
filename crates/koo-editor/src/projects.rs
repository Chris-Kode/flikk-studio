//! Project files from the user's side: New, Open, Save and Save As, and the
//! "Save changes?" question before anything replaces or closes a project
//! with unsaved changes. The files themselves are `persistence`'s.

use crate::state::EditorState;
use crate::ui::kit::DialogExt as _;
use gpui_kit::*;
use koo_store::project_file::{EXTENSION, sanitize};
use std::path::PathBuf;

actions!(koo, [NewProject, OpenProject, SaveProject, SaveProjectAs]);

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
    ]);
}

/// What replaces or closes the open project once its changes are settled.
#[derive(Clone)]
enum Then {
    New,
    Open(PathBuf),
    Quit,
}

pub fn new_project(state: &Entity<EditorState>, window: &mut Window, cx: &mut App) {
    settle(state, Then::New, window, cx);
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
    if state.read(cx).file.as_ref() == Some(&path) {
        return;
    }
    settle(state, Then::Open(path), window, cx);
}

/// Save to the project's file; a project never saved asks where.
pub fn save(state: &Entity<EditorState>, window: &mut Window, cx: &mut App) {
    save_then(state, None, window, cx);
}

/// Save to a new file, which the project continues in.
pub fn save_as(state: &Entity<EditorState>, window: &mut Window, cx: &mut App) {
    let target = state.clone();
    ask_path(state, window, cx, move |path, _, cx| {
        target.update(cx, |s, cx| s.save_project(path, cx));
    });
}

/// Quit, asking about unsaved changes first.
pub fn quit(state: &Entity<EditorState>, window: &mut Window, cx: &mut App) {
    settle(state, Then::Quit, window, cx);
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

/// Save, then (when it saved) do `then`.
fn save_then(state: &Entity<EditorState>, then: Option<Then>, window: &mut Window, cx: &mut App) {
    if let Some(path) = state.read(cx).file.clone() {
        if state.update(cx, |s, cx| s.save_project(path, cx))
            && let Some(then) = then
        {
            run(state, then, window, cx);
        }
        return;
    }
    let target = state.clone();
    ask_path(state, window, cx, move |path, window, cx| {
        if target.update(cx, |s, cx| s.save_project(path, cx))
            && let Some(then) = then
        {
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
        .file
        .as_ref()
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

/// Do `then` once unsaved changes are saved or let go, asking first.
fn settle(state: &Entity<EditorState>, then: Then, window: &mut Window, cx: &mut App) {
    if !state.read(cx).is_dirty() {
        run(state, then, window, cx);
        return;
    }
    if window.has_active_dialog(cx) {
        return;
    }
    let name = state.read(cx).bus.project().name.clone();
    let state = state.clone();
    window.open_confirm(cx, move |dialog, _, _| {
        let (save_state, discard_state) = (state.clone(), state.clone());
        let (save_then_do, discard_then_do) = (then.clone(), then.clone());
        dialog
            .title(format!("Save changes to “{name}”?"))
            .secondary("Don't Save", move |window, cx| {
                discard_state.update(cx, |s, _| s.discard_changes());
                run(&discard_state, discard_then_do.clone(), window, cx);
                true
            })
            .confirm("Save", move |window, cx| {
                save_then(&save_state, Some(save_then_do.clone()), window, cx);
                true
            })
    });
}

fn run(state: &Entity<EditorState>, then: Then, _: &mut Window, cx: &mut App) {
    match then {
        Then::New => state.update(cx, |s, cx| s.new_project(cx)),
        Then::Open(path) => {
            state.update(cx, |s, cx| s.open_project(&path, cx));
        }
        Then::Quit => cx.quit(),
    }
}
