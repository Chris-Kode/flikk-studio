//! The user's library: FX programs and presets shared by every project,
//! one file each under `paths::library()` (`.koofx`, `.koopreset`).
//!
//! Projects never point into the library. Using a program copies it into
//! the project ([`Project::fx`]) with [`FxAsset::origin`] naming the
//! library item: the project file stays complete on any machine, and
//! library edits reach a project only when the user updates its copy. A
//! preset carries the project programs its effects use, so it applies in
//! any project.
//!
//! Errors are messages for the user.

use crate::audiofx::AudioFx;
use crate::command::Command;
use crate::fx::{FxAsset, FxInstance, FxRef};
use crate::ids::{ClipId, EffectId, FxId, LibraryId};
use crate::model::{Grade, Project};
use crate::paths;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const PROGRAM_EXT: &str = "koofx";
pub const PRESET_EXT: &str = "koopreset";
const PROGRAM_FORMAT: &str = "koo-fx";
const PRESET_FORMAT: &str = "koo-preset";
/// Bumped when an older file needs an upgrade step on load.
const VERSION: u32 = 1;

pub type Result<T> = std::result::Result<T, String>;

/// A program in the library.
#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    pub id: LibraryId,
    /// Its `id` means nothing here; copies get their own.
    pub asset: FxAsset,
    pub path: PathBuf,
}

/// Effects, audio effects or a grade with their settings, to apply to
/// other clips.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub id: LibraryId,
    pub name: String,
    /// Video effects in stack order, with values and keyframes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<FxInstance>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub audio: Vec<AudioFx>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grade: Option<Grade>,
    /// The project programs `effects` use.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub programs: Vec<FxAsset>,
    #[serde(skip)]
    pub path: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct ProgramFile {
    format: String,
    version: u32,
    id: LibraryId,
    fx: FxAsset,
}

#[derive(Serialize, Deserialize)]
struct PresetFile {
    format: String,
    version: u32,
    #[serde(flatten)]
    preset: Preset,
}

/// Everything in the library, by name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Library {
    pub programs: Vec<Program>,
    pub presets: Vec<Preset>,
}

impl Library {
    /// Read `paths::library()`. Files that don't read are skipped.
    pub fn load() -> Self {
        let mut lib = Self::default();
        let files = std::fs::read_dir(paths::library())
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path());
        for path in files {
            let ext = path.extension().and_then(|e| e.to_str());
            let read = match ext {
                Some(PROGRAM_EXT) => read_program(&path).map(|p| lib.programs.push(p)),
                Some(PRESET_EXT) => read_preset(&path).map(|p| lib.presets.push(p)),
                _ => Ok(()),
            };
            if let Err(e) = read {
                eprintln!("koo-core: library: {e}");
            }
        }
        lib.programs.sort_by_key(|p| p.asset.name.to_lowercase());
        lib.presets.sort_by_key(|p| p.name.to_lowercase());
        lib
    }

    pub fn program(&self, id: LibraryId) -> Option<&Program> {
        self.programs.iter().find(|p| p.id == id)
    }

    pub fn preset(&self, id: LibraryId) -> Option<&Preset> {
        self.presets.iter().find(|p| p.id == id)
    }

    /// The library program a project program came from, if still there.
    pub fn origin_of(&self, asset: &FxAsset) -> Option<&Program> {
        self.program(asset.origin?)
    }

    /// A name no preset has yet: `base`, else `base 2`, `base 3`…
    pub fn free_preset_name(&self, base: &str) -> String {
        let taken = |n: &str| self.presets.iter().any(|p| p.name.eq_ignore_ascii_case(n));
        if !taken(base) {
            return base.to_owned();
        }
        (2..)
            .map(|i| format!("{base} {i}"))
            .find(|n| !taken(n))
            .unwrap_or_default()
    }
}

fn read_program(path: &Path) -> Result<Program> {
    let file: ProgramFile = read_json(path)?;
    check(path, &file.format, PROGRAM_FORMAT, file.version)?;
    Ok(Program {
        id: file.id,
        asset: file.fx,
        path: path.to_path_buf(),
    })
}

fn read_preset(path: &Path) -> Result<Preset> {
    let file: PresetFile = read_json(path)?;
    check(path, &file.format, PRESET_FORMAT, file.version)?;
    Ok(Preset {
        path: path.to_path_buf(),
        ..file.preset
    })
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = std::fs::read(path).map_err(|e| format!("can't read {}: {e}", name(path)))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("{} doesn't read: {e}", name(path)))
}

fn check(path: &Path, format: &str, want: &str, version: u32) -> Result<()> {
    if format != want {
        return Err(format!("{} is not a {want} file", name(path)));
    }
    if version > VERSION {
        return Err(format!(
            "{} was saved by a newer koo-editor; update to use it",
            name(path)
        ));
    }
    Ok(())
}

fn name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Write beside the target, then rename over it.
fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let fail = |e: &dyn std::fmt::Display| format!("can't save {}: {e}", name(path));
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| fail(&e))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| fail(&e))?;
    }
    let tmp = path.with_file_name(format!(".{}.tmp", name(path)));
    std::fs::write(&tmp, bytes)
        .and_then(|()| std::fs::rename(&tmp, path))
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            fail(&e)
        })
}

/// `name.ext` in the library, or `name 2.ext`… when another item has
/// that file. `own` is the item's current file, which it may keep.
fn file_for(name: &str, ext: &str, own: Option<&Path>) -> PathBuf {
    let dir = paths::library();
    let base = file_name_part(name);
    let free = |p: &Path| Some(p) == own || !p.exists();
    let first = dir.join(format!("{base}.{ext}"));
    if free(&first) {
        return first;
    }
    (2..)
        .map(|i| dir.join(format!("{base} {i}.{ext}")))
        .find(|p| free(p))
        .unwrap_or(first)
}

/// A file name part: no path separators or characters Windows refuses.
fn file_name_part(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let clean = clean.trim().trim_matches('.');
    if clean.is_empty() {
        "Untitled".into()
    } else {
        clean.into()
    }
}

/// Moved to `path`: drop the item's old file.
fn moved(old: Option<&Path>, path: &Path) {
    if let Some(old) = old.filter(|o| *o != path) {
        let _ = std::fs::remove_file(old);
    }
}

// ------------------------------------------------------------------ programs

/// Save a project program into the library: over the item it came from
/// when that is still there, else as a new item.
pub fn save_program(lib: &Library, asset: &FxAsset) -> Result<Program> {
    asset.validate()?;
    let existing = lib.origin_of(asset);
    let id = existing.map_or_else(LibraryId::new, |p| p.id);
    let old = existing.map(|p| p.path.as_path());
    let path = file_for(&asset.name, PROGRAM_EXT, old);
    let mut stored = asset.clone();
    stored.origin = None;
    write_json(
        &path,
        &ProgramFile {
            format: PROGRAM_FORMAT.into(),
            version: VERSION,
            id,
            fx: stored.clone(),
        },
    )?;
    moved(old, &path);
    Ok(Program {
        id,
        asset: stored,
        path,
    })
}

/// A copy of a library program under a new id and name.
pub fn duplicate_program(lib: &Library, program: &Program) -> Result<Program> {
    let mut asset = program.asset.clone();
    let taken = |n: &str| {
        lib.programs
            .iter()
            .any(|p| p.asset.name.eq_ignore_ascii_case(n))
    };
    let base = format!("{} copy", asset.name);
    asset.name = if taken(&base) {
        (2..)
            .map(|i| format!("{base} {i}"))
            .find(|n| !taken(n))
            .unwrap_or(base)
    } else {
        base
    };
    asset.origin = None;
    save_program(lib, &asset)
}

/// Rename a library program.
pub fn rename_program(lib: &Library, program: &Program, name: &str) -> Result<Program> {
    let mut asset = program.asset.clone();
    asset.name = name.trim().to_owned();
    asset.origin = Some(program.id);
    save_program(lib, &asset)
}

/// The project's copy of library `program`: one made before, else a new
/// copy to add with [`Command::SetFxAsset`].
pub fn copy_of(project: &Project, program: &Program) -> (FxRef, Option<FxAsset>) {
    if let Some(copy) = project.fx.iter().find(|a| a.origin == Some(program.id)) {
        return (copy.id.clone(), None);
    }
    let mut asset = program.asset.clone();
    asset.id = FxRef::Project(FxId::new());
    asset.origin = Some(program.id);
    (asset.id.clone(), Some(asset))
}

/// Whether two programs differ in anything but their ids and origin.
pub fn differs(a: &FxAsset, b: &FxAsset) -> bool {
    a.name != b.name
        || a.kind != b.kind
        || a.language != b.language
        || a.category != b.category
        || a.description != b.description
        || a.code != b.code
        || a.params != b.params
}

/// The project's copy brought up to the library program: same id and
/// origin, the library's code, name and parameters.
pub fn updated(copy: &FxAsset, program: &Program) -> FxAsset {
    FxAsset {
        id: copy.id.clone(),
        origin: Some(program.id),
        ..program.asset.clone()
    }
}

// ------------------------------------------------------------------- presets

impl Preset {
    /// Video effects of a clip in stack order, carrying the project
    /// programs they use.
    pub fn effects(name: &str, project: &Project, effects: Vec<FxInstance>) -> Self {
        let mut programs: Vec<FxAsset> = Vec::new();
        for e in &effects {
            if let FxRef::Project(_) = &e.fx
                && let Some(asset) = project.fx.iter().find(|a| a.id == e.fx)
                && !programs.iter().any(|p| p.id == asset.id)
            {
                programs.push(asset.clone());
            }
        }
        Self {
            programs,
            effects,
            ..Self::empty(name)
        }
    }

    pub fn audio(name: &str, audio: Vec<AudioFx>) -> Self {
        Self {
            audio,
            ..Self::empty(name)
        }
    }

    pub fn grade(name: &str, grade: Grade) -> Self {
        Self {
            grade: Some(grade),
            ..Self::empty(name)
        }
    }

    fn empty(name: &str) -> Self {
        Self {
            id: LibraryId::new(),
            name: name.trim().to_owned(),
            effects: Vec::new(),
            audio: Vec::new(),
            grade: None,
            programs: Vec::new(),
            path: PathBuf::new(),
        }
    }

    /// Takes picture clips (effects, grade).
    pub fn has_picture(&self) -> bool {
        !self.effects.is_empty() || self.grade.is_some()
    }

    /// Takes clips with sound.
    pub fn has_sound(&self) -> bool {
        !self.audio.is_empty()
    }
}

/// Save a preset into the library (new, or over its own file).
pub fn save_preset(preset: &Preset) -> Result<Preset> {
    if preset.name.trim().is_empty() {
        return Err("the preset needs a name".into());
    }
    let own = Some(preset.path.as_path()).filter(|p| !p.as_os_str().is_empty());
    let path = file_for(&preset.name, PRESET_EXT, own);
    write_json(
        &path,
        &PresetFile {
            format: PRESET_FORMAT.into(),
            version: VERSION,
            preset: preset.clone(),
        },
    )?;
    moved(own, &path);
    Ok(Preset {
        path,
        ..preset.clone()
    })
}

pub fn rename_preset(preset: &Preset, name: &str) -> Result<Preset> {
    save_preset(&Preset {
        name: name.trim().to_owned(),
        ..preset.clone()
    })
}

/// Commands that apply `preset`: the programs it carries copied into the
/// project (reusing equal ones there), its effects added after the others
/// on `picture` clips with its grade, and its audio effects after the
/// others on `sound` clips.
pub fn preset_commands(
    project: &Project,
    preset: &Preset,
    picture: &[ClipId],
    sound: &[ClipId],
) -> Vec<Command> {
    let mut commands = Vec::new();
    if !picture.is_empty() {
        let mut refs: HashMap<FxRef, FxRef> = HashMap::new();
        for program in &preset.programs {
            let same = project
                .fx
                .iter()
                .filter(|a| !differs(a, program))
                .max_by_key(|a| a.origin.is_some() && a.origin == program.origin);
            let to = match same {
                Some(a) => a.id.clone(),
                None => {
                    let mut copy = program.clone();
                    copy.id = FxRef::Project(FxId::new());
                    commands.push(Command::SetFxAsset {
                        asset: copy.clone(),
                    });
                    copy.id
                }
            };
            refs.insert(program.id.clone(), to);
        }
        for e in &preset.effects {
            let mut effect = e.clone();
            effect.id = EffectId::new();
            if let Some(to) = refs.get(&e.fx) {
                effect.fx = to.clone();
            }
            commands.push(Command::AddEffect {
                clips: picture.to_vec(),
                effect,
                index: None,
            });
        }
        if let Some(grade) = &preset.grade {
            commands.push(Command::SetGrade {
                clips: picture.to_vec(),
                grade: grade.clone(),
            });
        }
    }
    if !preset.audio.is_empty() {
        for id in sound {
            let Some((_, clip)) = project.timeline.clip(*id) else {
                continue;
            };
            let mut effects = clip.audio.effects.clone();
            effects.extend(preset.audio.iter().map(|a| AudioFx {
                id: EffectId::new(),
                ..a.clone()
            }));
            commands.push(Command::SetAudioEffects { clip: *id, effects });
        }
    }
    commands
}

// -------------------------------------------------------------------- shared

/// Remove an item's file.
pub fn remove(path: &Path) -> Result<()> {
    std::fs::remove_file(path).map_err(|e| format!("can't remove {}: {e}", name(path)))
}

/// Copy a `.koofx` or `.koopreset` file into the library. An item with the
/// same id is replaced.
pub fn import(lib: &Library, path: &Path) -> Result<()> {
    match path.extension().and_then(|e| e.to_str()) {
        Some(PROGRAM_EXT) => {
            let program = read_program(path)?;
            let mut asset = program.asset;
            asset.origin = lib.program(program.id).map(|p| p.id);
            match asset.origin {
                Some(_) => save_program(lib, &asset).map(|_| ()),
                None => {
                    let path = file_for(&asset.name, PROGRAM_EXT, None);
                    asset.validate()?;
                    write_json(
                        &path,
                        &ProgramFile {
                            format: PROGRAM_FORMAT.into(),
                            version: VERSION,
                            id: program.id,
                            fx: asset,
                        },
                    )
                }
            }
        }
        Some(PRESET_EXT) => {
            let mut preset = read_preset(path)?;
            preset.path = lib
                .preset(preset.id)
                .map(|p| p.path.clone())
                .unwrap_or_default();
            save_preset(&preset).map(|_| ())
        }
        _ => Err(format!(
            "{} is not a library file (.{PROGRAM_EXT} or .{PRESET_EXT})",
            name(path)
        )),
    }
}
