//! The user's library from the editor: programs and presets every project
//! can use (see `koo_core::library`). Using a program copies it into the
//! project first; saving one to the library links the project's copy to it.

use super::apply::{apply_fx, picture_clips, sound_clips};
use crate::state::EditorState;
use gpui_kit::*;
use koo_core::command::Command;
use koo_core::fx::{self, FxRef};
use koo_core::library::{self, Preset};
use koo_core::{LibraryId, model::Project};
use std::path::PathBuf;

/// The project's copy of library program `id`, copied in now if needed.
pub fn use_program(
    s: &mut EditorState,
    id: LibraryId,
    cx: &mut Context<EditorState>,
) -> Option<FxRef> {
    let program = s.library.program(id)?.clone();
    let (r, copy) = library::copy_of(s.bus.project(), &program);
    match copy {
        None => Some(r),
        Some(asset) => s.execute(Command::SetFxAsset { asset }, cx).map(|_| r),
    }
}

/// Apply library program `id` to the selection, like a built-in.
pub fn apply_program(s: &mut EditorState, id: LibraryId, cx: &mut Context<EditorState>) {
    if let Some(r) = use_program(s, id, cx) {
        apply_fx(s, &r, cx);
    }
}

/// Open library program `id` in the code editor: its copy in the project,
/// which "Save to Library" brings back.
pub fn edit_program(s: &mut EditorState, id: LibraryId, cx: &mut Context<EditorState>) {
    if let Some(r) = use_program(s, id, cx) {
        s.edit_fx(Some(r), cx);
    }
}

/// How a project program stands against the library.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    /// Not in the library (or its item was removed).
    None,
    /// Same as its library item.
    Same,
    /// It or its library item changed since.
    Differs,
}

pub fn link(s: &EditorState, r: &FxRef) -> Link {
    let Some(asset) = fx::asset(s.bus.project(), r) else {
        return Link::None;
    };
    match s.library.origin_of(asset) {
        None => Link::None,
        Some(p) if library::differs(asset, &p.asset) => Link::Differs,
        Some(_) => Link::Same,
    }
}

/// Save project program `r` to the library (over the item it came from,
/// if any) and link the project's copy to it.
pub fn save_program(s: &mut EditorState, r: &FxRef, cx: &mut Context<EditorState>) {
    let Some(asset) = fx::asset(s.bus.project(), r).cloned() else {
        return;
    };
    match library::save_program(&s.library, &asset) {
        Ok(program) => {
            s.reload_library(cx);
            if asset.origin != Some(program.id) {
                let mut linked = asset;
                linked.origin = Some(program.id);
                s.execute(Command::SetFxAsset { asset: linked }, cx);
            }
        }
        Err(e) => fail(s, e, cx),
    }
}

/// Link project program `text` (an id or `@fx` token) to the library
/// item saved from it, so its menu offers updates instead of adding again.
pub fn link_to_library(s: &mut EditorState, text: &str, cx: &mut Context<EditorState>) {
    let project = s.bus.project();
    let Some(asset) = fx::find(project, text).and_then(|r| fx::asset(project, &r)) else {
        return;
    };
    if asset
        .origin
        .is_some_and(|id| s.library.program(id).is_some())
    {
        return;
    }
    let Some(program) = s
        .library
        .programs
        .iter()
        .find(|p| !library::differs(asset, &p.asset))
    else {
        return;
    };
    let mut linked = asset.clone();
    linked.origin = Some(program.id);
    s.execute(Command::SetFxAsset { asset: linked }, cx);
}

/// Bring project program `r` up to its library item.
pub fn update_from_library(s: &mut EditorState, r: &FxRef, cx: &mut Context<EditorState>) {
    let Some(asset) = fx::asset(s.bus.project(), r) else {
        return;
    };
    let Some(program) = s.library.origin_of(asset) else {
        return;
    };
    let asset = library::updated(asset, program);
    s.execute(Command::SetFxAsset { asset }, cx);
}

pub fn duplicate_program(s: &mut EditorState, id: LibraryId, cx: &mut Context<EditorState>) {
    let Some(program) = s.library.program(id).cloned() else {
        return;
    };
    match library::duplicate_program(&s.library, &program) {
        Ok(_) => s.reload_library(cx),
        Err(e) => fail(s, e, cx),
    }
}

pub fn rename(s: &mut EditorState, id: LibraryId, name: &str, cx: &mut Context<EditorState>) {
    if name.trim().is_empty() {
        return;
    }
    let renamed = if let Some(program) = s.library.program(id).cloned() {
        library::rename_program(&s.library, &program, name).map(|_| ())
    } else if let Some(preset) = s.library.preset(id).cloned() {
        library::rename_preset(&preset, name).map(|_| ())
    } else {
        return;
    };
    match renamed {
        Ok(()) => s.reload_library(cx),
        Err(e) => fail(s, e, cx),
    }
}

/// Remove a program or preset from the library. Projects keep their
/// copies.
pub fn remove(s: &mut EditorState, id: LibraryId, cx: &mut Context<EditorState>) {
    let Some(path) = file_of(s, id) else {
        return;
    };
    match library::remove(&path) {
        Ok(()) => s.reload_library(cx),
        Err(e) => fail(s, e, cx),
    }
}

/// The file of a program or preset.
pub fn file_of(s: &EditorState, id: LibraryId) -> Option<PathBuf> {
    s.library
        .program(id)
        .map(|p| p.path.clone())
        .or_else(|| s.library.preset(id).map(|p| p.path.clone()))
}

/// Copy `.koofx` and `.koopreset` files into the library.
pub fn import(s: &mut EditorState, paths: &[PathBuf], cx: &mut Context<EditorState>) {
    let errors: Vec<String> = paths
        .iter()
        .filter_map(|p| library::import(&s.library, p).err())
        .collect();
    s.reload_library(cx);
    if !errors.is_empty() {
        fail(s, errors.join(" · "), cx);
    }
}

/// Save a preset made from the project and show it in the library.
pub fn save_preset(s: &mut EditorState, preset: Preset, cx: &mut Context<EditorState>) {
    match library::save_preset(&preset) {
        Ok(_) => s.reload_library(cx),
        Err(e) => fail(s, e, cx),
    }
}

/// A preset of `effects` from `clip`'s stack, named after the program when
/// there is one effect.
pub fn effects_preset(s: &EditorState, effects: Vec<fx::FxInstance>) -> Preset {
    let project = s.bus.project();
    let base = match effects.as_slice() {
        [one] => fx::asset(project, &one.fx).map_or("Effect".into(), |a| a.name.clone()),
        _ => "Effects".into(),
    };
    Preset::effects(&s.library.free_preset_name(&base), project, effects)
}

/// Apply preset `id`: effects and grade to the selected picture clips,
/// audio effects to their sound, as one undo step.
pub fn apply_preset(s: &mut EditorState, id: LibraryId, cx: &mut Context<EditorState>) {
    let Some(preset) = s.library.preset(id).cloned() else {
        return;
    };
    let picture = if preset.has_picture() {
        picture_clips(s)
    } else {
        Vec::new()
    };
    let sound = if preset.has_sound() {
        sound_clips(s)
    } else {
        Vec::new()
    };
    apply_preset_to(s, &preset, &picture, &sound, cx);
}

/// Apply `preset` to the given clips.
pub fn apply_preset_to(
    s: &mut EditorState,
    preset: &Preset,
    picture: &[koo_core::ClipId],
    sound: &[koo_core::ClipId],
    cx: &mut Context<EditorState>,
) {
    let commands = library::preset_commands(s.bus.project(), preset, picture, sound);
    if commands.is_empty() {
        let what = if preset.has_picture() {
            "Select a picture clip"
        } else {
            "Select a clip with sound"
        };
        s.status = Some(what.into());
        cx.notify();
        return;
    }
    s.execute_batch(commands, "apply_preset", cx);
}

/// Library programs of `kind` the project has no copy of yet (copies are
/// listed with the project's programs).
pub fn programs_to_add(
    project: &Project,
    lib: &library::Library,
    kind: fx::FxKind,
) -> Vec<(LibraryId, String)> {
    lib.programs
        .iter()
        .filter(|p| p.asset.kind == kind)
        .filter(|p| !project.fx.iter().any(|a| a.origin == Some(p.id)))
        .map(|p| (p.id, p.asset.name.clone()))
        .collect()
}

fn fail(s: &mut EditorState, message: String, cx: &mut Context<EditorState>) {
    s.status = Some(message);
    cx.notify();
}
