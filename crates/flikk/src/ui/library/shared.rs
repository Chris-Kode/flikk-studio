//! The user's library from the editor: programs, presets and sounds every
//! project can use (see `flikk_core::library`). Using a program copies it into
//! the project first; saving one to the library links the project's copy to
//! it. Using a sound adds it to the pool first.

use super::apply::{apply_fx, picture_clips, sound_clips};
use crate::state::EditorState;
use crate::ui::media_pool::import::{build_item, is_audio};
use flikk_core::command::{Command, InsertMode};
use flikk_core::fx::{self, FxRef};
use flikk_core::library::{self, Preset};
use flikk_core::model::{MediaItem, TrackKind};
use flikk_core::{LibraryId, MediaId, Time, TimeRange, TrackId, model::Project};
use gpui_kit::*;
use std::collections::HashSet;
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
    } else if let Some(sound) = s.library.sound(id).cloned() {
        library::rename_sound(&sound, name).map(|_| ())
    } else if s.library.voice(id).is_some() {
        return s.rename_saved_voice(id, name, cx);
    } else {
        return;
    };
    match renamed {
        Ok(()) => s.reload_library(cx),
        Err(e) => fail(s, e, cx),
    }
}

/// Remove a program, preset, sound or voice from the library. Projects keep
/// their copies, sounds their audio, and voices stay on the account.
pub fn remove(s: &mut EditorState, id: LibraryId, cx: &mut Context<EditorState>) {
    if s.library.voice(id).is_some() {
        return s.forget_voice(id, cx);
    }
    let Some(path) = file_of(s, id) else {
        return;
    };
    match library::remove(&path) {
        Ok(()) => s.reload_library(cx),
        Err(e) => fail(s, e, cx),
    }
}

/// The file of a program, preset, sound or voice.
pub fn file_of(s: &EditorState, id: LibraryId) -> Option<PathBuf> {
    s.library
        .program(id)
        .map(|p| p.path.clone())
        .or_else(|| s.library.preset(id).map(|p| p.path.clone()))
        .or_else(|| s.library.sound(id).map(|p| p.path.clone()))
        .or_else(|| s.library.voice(id).map(|v| v.path.clone()))
}

/// Copy `.flikkfx` and `.flikkpreset` files into the library, and audio files
/// as sounds.
pub fn import(s: &mut EditorState, paths: &[PathBuf], cx: &mut Context<EditorState>) {
    let (audio, files): (Vec<_>, Vec<_>) = paths.iter().cloned().partition(|p| is_audio(p));
    let errors: Vec<String> = files
        .iter()
        .filter_map(|p| library::import(&s.library, p).err())
        .collect();
    s.reload_library(cx);
    if !errors.is_empty() {
        fail(s, errors.join(" · "), cx);
    }
    if !audio.is_empty() {
        let media = s.services.media.clone();
        save_sounds(
            s,
            move || {
                let build = |p| build_item(p, None, media.as_deref());
                audio.into_iter().filter_map(build).collect()
            },
            Vec::new(),
            cx,
        );
    }
}

/// Pool items being copied into the library: the library lists them only
/// once the copy is done, so until then they can't be saved again.
#[derive(Default)]
struct SavingSounds(HashSet<MediaId>);

impl Global for SavingSounds {}

/// Save pool item `id` to the library.
pub fn save_sound(s: &mut EditorState, id: MediaId, cx: &mut Context<EditorState>) {
    if !can_save_sound(s, id, cx) {
        return;
    }
    if let Some(item) = s.bus.project().media(id).cloned() {
        cx.default_global::<SavingSounds>().0.insert(id);
        save_sounds(s, move || vec![item], vec![id], cx);
    }
}

/// Whether pool item `id` can go to the library: audio that is there to
/// copy and not in it, or on its way, yet.
pub fn can_save_sound(s: &EditorState, id: MediaId, cx: &App) -> bool {
    let saving = cx
        .try_global::<SavingSounds>()
        .is_some_and(|g| g.0.contains(&id));
    !saving
        && s.bus.project().media(id).is_some_and(|item| {
            item.kind == flikk_core::model::MediaKind::Audio
                && !s.is_offline(id)
                && s.library.sound_of(item).is_none()
        })
}

/// Off the UI thread: the audio is copied in, then pool items `saving`
/// can be saved again.
fn save_sounds(
    s: &EditorState,
    items: impl FnOnce() -> Vec<MediaItem> + Send + 'static,
    saving: Vec<MediaId>,
    cx: &mut Context<EditorState>,
) {
    let lib = s.library.clone();
    let saved = cx.background_executor().spawn(async move {
        let mut lib = lib;
        let mut errors = Vec::new();
        for item in items() {
            match library::save_sound(&lib, &item) {
                Ok(sound) => lib.sounds.push(sound),
                Err(e) => errors.push(e),
            }
        }
        errors
    });
    cx.spawn(async move |this, cx| {
        let errors = saved.await;
        let _ = this.update(cx, |s, cx| {
            let pending = &mut cx.default_global::<SavingSounds>().0;
            for id in &saving {
                pending.remove(id);
            }
            s.reload_library(cx);
            if !errors.is_empty() {
                fail(s, errors.join(" · "), cx);
            }
        });
    })
    .detach();
}

/// The project's media item for library sound `id`, added to the pool now
/// if needed.
pub fn use_sound(
    s: &mut EditorState,
    id: LibraryId,
    cx: &mut Context<EditorState>,
) -> Option<MediaId> {
    let sound = s.library.sound(id)?.clone();
    let (media, commands) = library::sound_media(s.bus.project(), &sound);
    if commands.is_empty() {
        return Some(media);
    }
    s.execute_batch(commands, "Add sound", cx).map(|_| media)
}

/// Place library sound `id` at the playhead, on an audio track with nothing
/// over its length, else on a new one.
pub fn place_sound(s: &mut EditorState, id: LibraryId, cx: &mut Context<EditorState>) {
    let Some(sound) = s.library.sound(id).cloned() else {
        return;
    };
    let length = sound
        .duration()
        .filter(|d| d.0 > 0)
        .unwrap_or(Time::from_secs_f64(5.));
    let at = s.playhead;
    let Some(track) = free_audio_track(s, TimeRange::new(at, at + length), cx) else {
        return;
    };
    let (media, mut commands) = library::sound_media(s.bus.project(), &sound);
    commands.push(Command::InsertClip {
        media,
        source: TimeRange::new(Time::ZERO, length),
        track,
        at,
        mode: InsertMode::Overwrite,
    });
    s.edit_batch(commands, "Add sound", cx);
}

/// An audio track free over `span`, added if there is none.
fn free_audio_track(
    s: &mut EditorState,
    span: TimeRange,
    cx: &mut Context<EditorState>,
) -> Option<TrackId> {
    let tracks = &s.bus.project().timeline.tracks;
    let before: Vec<TrackId> = tracks.iter().map(|t| t.id).collect();
    if let Some(track) = tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Audio && !t.locked)
        .find(|t| t.clips.iter().all(|c| !c.range().overlaps(&span)))
    {
        return Some(track.id);
    }
    let command = Command::AddTrack {
        kind: TrackKind::Audio,
        index: before.len(),
        name: s.bus.project().timeline.new_track_slot(TrackKind::Audio).1,
    };
    s.execute(command, cx)?;
    let tracks = &s.bus.project().timeline.tracks;
    tracks
        .iter()
        .find(|t| !before.contains(&t.id))
        .map(|t| t.id)
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
    picture: &[flikk_core::ClipId],
    sound: &[flikk_core::ClipId],
    cx: &mut Context<EditorState>,
) {
    let commands = library::preset_commands(s.bus.project(), preset, picture, sound);
    if commands.is_empty() {
        let what = if preset.has_picture() {
            "Select a picture clip"
        } else {
            "Select a clip with sound"
        };
        s.say(what, cx);
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
    s.say(message, cx);
}
