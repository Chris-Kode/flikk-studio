//! The user's library: FX programs, presets, sounds and voices shared by
//! every project, one file each under `paths::library()` (`.flikkfx`,
//! `.flikkpreset`, `.flikksound`, `.flikkvoice`).
//!
//! Projects never point into the library. Using a program copies it into
//! the project ([`Project::fx`]) with [`FxAsset::origin`] naming the
//! library item: the project file stays complete on any machine, and
//! library edits reach a project only when the user updates its copy. A
//! preset carries the project programs its effects use, so it applies in
//! any project.
//!
//! A sound's audio lives in `paths::assets()`, and a project using it holds
//! a media item on that file, as it does for any source. Removing a sound
//! leaves the file, so projects using it stay online.
//!
//! A voice lives on the provider's account; its file here remembers how it
//! was made, how it reads best and a sample of it, so it is listed, heard
//! and made again without a request.
//!
//! Errors are messages for the user.

use crate::audiofx::AudioFx;
use crate::cloud::{self, Capability, GenerationInfo, Reading, VoiceOrigin};
use crate::command::Command;
use crate::fx::{FxAsset, FxInstance, FxRef};
use crate::ids::{ClipId, EffectId, FxId, LibraryId, MediaId, SpeakerId};
use crate::model::{Fingerprint, Grade, MediaInfo, MediaItem, MediaKind, Project};
use crate::paths;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const PROGRAM_EXT: &str = "flikkfx";
pub const PRESET_EXT: &str = "flikkpreset";
pub const SOUND_EXT: &str = "flikksound";
pub const VOICE_EXT: &str = "flikkvoice";
/// Name of the bin sounds from the library go into.
pub const LIBRARY_BIN: &str = "Library";
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

/// A sound or piece of music any project can use.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sound {
    pub id: LibraryId,
    pub name: String,
    /// The audio's file, relative to `paths::assets()`.
    pub file: String,
    pub fingerprint: Option<Fingerprint>,
    pub info: Option<MediaInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// How a provider made it, when it was generated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated: Option<Box<GenerationInfo>>,
    #[serde(skip)]
    pub path: PathBuf,
}

/// A provider's voice the user made, added or keeps, to read voiceovers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedVoice {
    pub id: LibraryId,
    pub name: String,
    /// Provider id: "elevenlabs".
    pub provider: String,
    /// The provider's id for it.
    pub voice: String,
    pub origin: VoiceOrigin,
    /// What it is like: "calm", "british", "narration".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub traits: Vec<String>,
    /// How it reads unless told otherwise.
    #[serde(default = "default_stability")]
    pub stability: f32,
    #[serde(default = "default_similarity")]
    pub similarity: f32,
    /// A sample of it, relative to `paths::assets()`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample: Option<String>,
    /// The sample's length and format, to play it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_info: Option<MediaInfo>,
    /// Speakers in recordings whose voice it is: words fixed in what they
    /// said are read with it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub speakers: Vec<VoiceSpeaker>,
    #[serde(skip)]
    pub path: PathBuf,
}

/// A speaker of one recording, as its analysis numbers them; `None` when
/// voices weren't told apart in it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceSpeaker {
    pub fingerprint: Fingerprint,
    #[serde(default)]
    pub speaker: Option<SpeakerId>,
}

fn default_stability() -> f32 {
    Reading::STABILITY
}

fn default_similarity() -> f32 {
    Reading::SIMILARITY
}

/// A kind of library item, kept as one JSON file of its own.
trait Stored: Clone {
    /// The extension of its files.
    const EXT: &'static str;
    /// The `format` its files name.
    const FORMAT: &'static str;
    /// What its files hold.
    type File: Serialize + DeserializeOwned + Header;
    /// The file holding the item.
    fn file(&self) -> Self::File;
    /// The item `file`, read from `path`, holds; `Err` when it can't be
    /// used.
    fn open(file: Self::File, path: &Path) -> Result<Self>;
    /// What its file is named after.
    fn name(&self) -> &str;
    /// Its file; empty for an item not saved yet.
    fn path(&self) -> &Path;
    /// The item, kept in `path`.
    fn at(self, path: PathBuf) -> Self;
}

/// What every library file starts with.
trait Header {
    /// The file's `format` and `version`.
    fn header(&self) -> (&str, u32);
}

#[derive(Serialize, Deserialize)]
struct ProgramFile {
    format: String,
    version: u32,
    id: LibraryId,
    fx: FxAsset,
}

/// The file of a preset, sound or voice: its `format` and `version`, then
/// the item's own fields.
#[derive(Serialize, Deserialize)]
struct ItemFile<T> {
    format: String,
    version: u32,
    #[serde(flatten)]
    item: T,
}

impl Header for ProgramFile {
    fn header(&self) -> (&str, u32) {
        (&self.format, self.version)
    }
}

impl<T> Header for ItemFile<T> {
    fn header(&self) -> (&str, u32) {
        (&self.format, self.version)
    }
}

impl Stored for Program {
    const EXT: &'static str = PROGRAM_EXT;
    const FORMAT: &'static str = "flikk-fx";
    type File = ProgramFile;

    fn file(&self) -> ProgramFile {
        ProgramFile {
            format: Self::FORMAT.into(),
            version: VERSION,
            id: self.id,
            fx: self.asset.clone(),
        }
    }

    fn open(file: ProgramFile, path: &Path) -> Result<Self> {
        Ok(Self {
            id: file.id,
            asset: file.fx,
            path: path.to_path_buf(),
        })
    }

    fn name(&self) -> &str {
        &self.asset.name
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn at(self, path: PathBuf) -> Self {
        Self { path, ..self }
    }
}

impl Stored for Preset {
    const EXT: &'static str = PRESET_EXT;
    const FORMAT: &'static str = "flikk-preset";
    type File = ItemFile<Self>;

    fn file(&self) -> ItemFile<Self> {
        ItemFile {
            format: Self::FORMAT.into(),
            version: VERSION,
            item: self.clone(),
        }
    }

    fn open(file: ItemFile<Self>, path: &Path) -> Result<Self> {
        Ok(file.item.at(path.to_path_buf()))
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn at(self, path: PathBuf) -> Self {
        Self { path, ..self }
    }
}

impl Stored for Sound {
    const EXT: &'static str = SOUND_EXT;
    const FORMAT: &'static str = "flikk-sound";
    type File = ItemFile<Self>;

    fn file(&self) -> ItemFile<Self> {
        ItemFile {
            format: Self::FORMAT.into(),
            version: VERSION,
            item: self.clone(),
        }
    }

    fn open(file: ItemFile<Self>, path: &Path) -> Result<Self> {
        let sound = file.item.at(path.to_path_buf());
        if !sound.audio().is_file() {
            return Err(format!("the audio of {} is missing", name(path)));
        }
        Ok(sound)
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn at(self, path: PathBuf) -> Self {
        Self { path, ..self }
    }
}

impl Stored for SavedVoice {
    const EXT: &'static str = VOICE_EXT;
    const FORMAT: &'static str = "flikk-voice";
    type File = ItemFile<Self>;

    fn file(&self) -> ItemFile<Self> {
        ItemFile {
            format: Self::FORMAT.into(),
            version: VERSION,
            item: self.clone(),
        }
    }

    fn open(file: ItemFile<Self>, path: &Path) -> Result<Self> {
        Ok(file.item.at(path.to_path_buf()))
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn at(self, path: PathBuf) -> Self {
        Self { path, ..self }
    }
}

/// Everything in the library, by name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Library {
    pub programs: Vec<Program>,
    pub presets: Vec<Preset>,
    pub sounds: Vec<Sound>,
    pub voices: Vec<SavedVoice>,
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
                Some(PROGRAM_EXT) => read(&path).map(|p| lib.programs.push(p)),
                Some(PRESET_EXT) => read(&path).map(|p| lib.presets.push(p)),
                Some(SOUND_EXT) => read(&path).map(|s| lib.sounds.push(s)),
                Some(VOICE_EXT) => read(&path).map(|v| lib.voices.push(v)),
                _ => Ok(()),
            };
            if let Err(e) = read {
                eprintln!("flikk-core: library: {e}");
            }
        }
        lib.programs.sort_by_key(|p| p.asset.name.to_lowercase());
        lib.presets.sort_by_key(|p| p.name.to_lowercase());
        lib.sounds.sort_by_key(|s| s.name.to_lowercase());
        lib.voices.sort_by_key(|v| v.name.to_lowercase());
        lib
    }

    pub fn program(&self, id: LibraryId) -> Option<&Program> {
        self.programs.iter().find(|p| p.id == id)
    }

    pub fn preset(&self, id: LibraryId) -> Option<&Preset> {
        self.presets.iter().find(|p| p.id == id)
    }

    pub fn sound(&self, id: LibraryId) -> Option<&Sound> {
        self.sounds.iter().find(|s| s.id == id)
    }

    pub fn voice(&self, id: LibraryId) -> Option<&SavedVoice> {
        self.voices.iter().find(|v| v.id == id)
    }

    /// `provider`'s saved voice that `who` speaks with.
    pub fn voice_of_speaker(&self, provider: &str, who: &VoiceSpeaker) -> Option<&SavedVoice> {
        self.voices
            .iter()
            .find(|v| v.provider == provider && v.speakers.contains(who))
    }

    /// The saved voice that is `provider`'s `voice`.
    pub fn voice_of(&self, provider: &str, voice: &str) -> Option<&SavedVoice> {
        self.voices
            .iter()
            .find(|v| v.provider == provider && v.voice == voice)
    }

    /// The sound holding the same audio as `item`.
    pub fn sound_of(&self, item: &MediaItem) -> Option<&Sound> {
        self.sounds
            .iter()
            .find(|s| s.audio() == item.path || same_audio(&s.fingerprint, &item.fingerprint))
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

/// The item in the library file at `path`.
fn read<T: Stored>(path: &Path) -> Result<T> {
    let bytes = std::fs::read(path).map_err(|e| format!("can't read {}: {e}", name(path)))?;
    let file: T::File =
        serde_json::from_slice(&bytes).map_err(|e| format!("{} doesn't read: {e}", name(path)))?;
    let (format, version) = file.header();
    if format != T::FORMAT {
        return Err(format!("{} is not a {} file", name(path), T::FORMAT));
    }
    if version > VERSION {
        return Err(format!(
            "{} was saved by a newer flikk; update to use it",
            name(path)
        ));
    }
    T::open(file, path)
}

/// Write `item` to its own file, under a new name when it was renamed, or
/// to a new file when it has none: the item, kept in the file written.
fn store<T: Stored>(item: &T) -> Result<T> {
    let own = Some(item.path()).filter(|p| !p.as_os_str().is_empty());
    let file = file_for(item.name(), T::EXT, own)?;
    write_json(file.path(), &item.file())?;
    let path = file.written();
    moved(own, &path);
    Ok(item.clone().at(path))
}

fn name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Write whole or not at all, and durably: the library is the user's data.
fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let fail = |e: &dyn std::fmt::Display| format!("can't save {}: {e}", name(path));
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| fail(&e))?;
    flikk_os::fs::write_durable(path, &bytes).map_err(|e| fail(&e))
}

/// `title.ext` in the library, or `title 2.ext`… when another item has
/// that file. `own` is the item's current file, which it may keep. A new
/// name is claimed by creating it empty, so two saves never pick the same
/// one.
fn file_for(title: &str, ext: &str, own: Option<&Path>) -> Result<FileFor> {
    let dir = paths::library();
    let base = file_name_part(title);
    let mut n = 1;
    loop {
        let path = match n {
            1 => dir.join(format!("{base}.{ext}")),
            n => dir.join(format!("{base} {n}.{ext}")),
        };
        n += 1;
        if Some(path.as_path()) == own {
            return Ok(FileFor::Own(path));
        }
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => return Ok(FileFor::Claimed(path)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(format!("can't save {}: {e}", name(&path))),
        }
    }
}

/// The file an item is written to, from [`file_for`].
enum FileFor {
    /// The item's own file, written over.
    Own(PathBuf),
    /// A new name, held by an empty file of ours until the item is written
    /// over it; removed if it never is.
    Claimed(PathBuf),
}

impl FileFor {
    fn path(&self) -> &Path {
        match self {
            Self::Own(path) | Self::Claimed(path) => path,
        }
    }

    /// The file, now holding the item: kept.
    fn written(self) -> PathBuf {
        // The claim is the item's file now: drop must not remove it.
        let mut kept = std::mem::ManuallyDrop::new(self);
        match &mut *kept {
            Self::Own(path) | Self::Claimed(path) => std::mem::take(path),
        }
    }
}

impl Drop for FileFor {
    fn drop(&mut self) {
        if let Self::Claimed(path) = self {
            let _ = std::fs::remove_file(path);
        }
    }
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
    let mut stored = asset.clone();
    stored.origin = None;
    store(&Program {
        id: existing.map_or_else(LibraryId::new, |p| p.id),
        asset: stored,
        path: existing.map(|p| p.path.clone()).unwrap_or_default(),
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
    store(preset)
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

// -------------------------------------------------------------------- sounds

fn same_audio(a: &Option<Fingerprint>, b: &Option<Fingerprint>) -> bool {
    a.is_some() && a == b
}

impl Sound {
    /// The audio file.
    pub fn audio(&self) -> PathBuf {
        paths::assets().join(&self.file)
    }

    /// Group in the library.
    pub fn category(&self) -> &'static str {
        sound_category(self.generated.as_deref())
    }

    pub fn duration(&self) -> Option<crate::Time> {
        self.info.as_ref().map(|i| i.duration)
    }

    /// A media item on its audio; not in a project or a bin yet.
    pub fn item(&self) -> MediaItem {
        MediaItem {
            id: MediaId::new(),
            path: self.audio(),
            kind: MediaKind::Audio,
            fingerprint: self.fingerprint.clone(),
            info: self.info.clone(),
            bin: None,
            tags: self.tags.clone(),
            proxy: None,
            generated: self.generated.clone(),
            name: None,
            tone_map: Default::default(),
        }
    }
}

/// Group of audio made as `generated`, plain audio when it wasn't.
pub fn sound_category(generated: Option<&GenerationInfo>) -> &'static str {
    match generated.map(|g| g.capability) {
        Some(Capability::Music) => "Music",
        Some(Capability::SoundEffects) => "Sound Effects",
        Some(Capability::Voiceover) => "Voiceover",
        _ => "Audio",
    }
}

/// What to call `item` in the library: the start of what was asked of a
/// provider, else its file name.
fn sound_name(item: &MediaItem) -> String {
    match &item.generated {
        Some(g) => g
            .prompt
            .split_whitespace()
            .take(6)
            .collect::<Vec<_>>()
            .join(" ")
            .trim_end_matches(|c: char| c.is_ascii_punctuation())
            .to_owned(),
        None => item
            .path
            .file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    }
}

/// Copy `from` to `to`, kept if already there: the same audio. Slow for
/// long audio.
fn copy_audio(from: &Path, to: &Path) -> Result<()> {
    if to.is_file() {
        return Ok(());
    }
    flikk_os::fs::write_atomic_with(to, |file| {
        std::io::copy(&mut std::fs::File::open(from)?, file)?;
        // On disk before it is named: sounds and samples are the user's
        // data, which must survive a power cut.
        file.sync_all()
    })
    .map_err(|e| format!("can't copy {}: {e}", name(from)))
}

/// Put audio `item` in the library: its file copied in under a folder named
/// by its content, keeping the name the pool shows. A sound with the same
/// audio is returned as it is.
pub fn save_sound(lib: &Library, item: &MediaItem) -> Result<Sound> {
    if item.kind != MediaKind::Audio {
        return Err(format!("{} is not a sound", name(&item.path)));
    }
    if let Some(sound) = lib.sound_of(item) {
        return Ok(sound.clone());
    }
    let id = LibraryId::new();
    let folder = item
        .fingerprint
        .as_ref()
        .map_or_else(|| id.0.to_string(), |f| file_name_part(&f.0));
    let file = format!("{folder}/{}", name(&item.path));
    copy_audio(&item.path, &paths::assets().join(&file))?;
    store(&Sound {
        id,
        name: sound_name(item),
        file,
        fingerprint: item.fingerprint.clone(),
        info: item.info.clone(),
        tags: item.tags.clone(),
        generated: item.generated.clone(),
        path: PathBuf::new(),
    })
}

pub fn rename_sound(sound: &Sound, name: &str) -> Result<Sound> {
    store(&Sound {
        name: name.trim().to_owned(),
        ..sound.clone()
    })
}

/// The project's media item for `sound`: one with the same audio, else a
/// new one and the commands that add it to the [`LIBRARY_BIN`] bin.
pub fn sound_media(project: &Project, sound: &Sound) -> (MediaId, Vec<Command>) {
    let audio = sound.audio();
    if let Some(item) = project
        .media
        .iter()
        .find(|m| m.path == audio || same_audio(&m.fingerprint, &sound.fingerprint))
    {
        return (item.id, Vec::new());
    }
    let item = sound.item();
    (item.id, cloud::pool_commands(project, item, LIBRARY_BIN))
}

// -------------------------------------------------------------------- voices

impl SavedVoice {
    /// A new entry for `provider`'s `voice`, read with the usual settings.
    pub fn new(provider: &str, voice: &cloud::Voice, origin: VoiceOrigin) -> Self {
        Self {
            id: LibraryId::new(),
            name: voice.name.clone(),
            provider: provider.to_owned(),
            voice: voice.id.clone(),
            origin,
            traits: voice.traits.clone(),
            stability: Reading::STABILITY,
            similarity: Reading::SIMILARITY,
            sample: None,
            sample_info: None,
            speakers: Vec::new(),
            path: PathBuf::new(),
        }
    }

    /// Its sample's file, when it has one.
    pub fn sample_file(&self) -> Option<PathBuf> {
        let path = paths::assets().join(self.sample.as_ref()?);
        path.is_file().then_some(path)
    }

    /// Its sample as an item to listen to: not in any pool.
    pub fn sample_item(&self) -> Option<MediaItem> {
        Some(MediaItem {
            id: MediaId::new(),
            path: self.sample_file()?,
            kind: MediaKind::Audio,
            fingerprint: None,
            info: Some(self.sample_info.clone()?),
            bin: None,
            tags: Vec::new(),
            proxy: None,
            generated: None,
            name: None,
            tone_map: Default::default(),
        })
    }

    /// How it reads a voiceover unless told otherwise.
    pub fn reading(&self) -> Reading {
        Reading {
            voice: self.voice.clone(),
            voice_name: self.name.clone(),
            stability: self.stability,
            similarity: self.similarity,
        }
    }

    /// Group in the library.
    pub fn category(&self) -> &'static str {
        match self.origin {
            VoiceOrigin::Designed { .. } => "Designed",
            VoiceOrigin::Cloned { .. } => "Cloned",
            VoiceOrigin::Shared { .. } => "Voice Library",
            VoiceOrigin::Account => "Account",
        }
    }

    /// What it was made from, for a tooltip.
    pub fn description(&self) -> String {
        match &self.origin {
            VoiceOrigin::Designed { description, .. } => description.clone(),
            VoiceOrigin::Cloned { sources } => sources.join(", "),
            VoiceOrigin::Shared { name, .. } => name.clone(),
            VoiceOrigin::Account => self.traits.join(", "),
        }
    }
}

/// Save `voice` to the library: over the entry for the same provider voice
/// when there is one (keeping its id), else as a new one. `sample`, an
/// item on a recording of it, is copied in as its sample.
pub fn save_voice(
    lib: &Library,
    voice: &SavedVoice,
    sample: Option<&MediaItem>,
) -> Result<SavedVoice> {
    if voice.name.trim().is_empty() {
        return Err("the voice needs a name".into());
    }
    let existing = lib.voice_of(&voice.provider, &voice.voice);
    let mut stored = voice.clone();
    if let Some(e) = existing {
        stored.id = e.id;
        if stored.path.as_os_str().is_empty() {
            stored.path = e.path.clone();
        }
        if stored.sample.is_none() {
            stored.sample = e.sample.clone();
            stored.sample_info = e.sample_info.clone();
        }
        if stored.speakers.is_empty() {
            stored.speakers = e.speakers.clone();
        }
    }
    if let Some(item) = sample {
        let sample = item.path.as_path();
        let ext = sample.extension().and_then(|e| e.to_str()).unwrap_or("mp3");
        let file = format!(
            "voices/{}-{}.{ext}",
            file_name_part(&stored.provider),
            file_name_part(&stored.voice)
        );
        let to = paths::assets().join(&file);
        let _ = std::fs::remove_file(&to);
        copy_audio(sample, &to)?;
        stored.sample = Some(file);
        stored.sample_info = item.info.clone();
    }
    store(&stored)
}

pub fn rename_voice(voice: &SavedVoice, name: &str) -> Result<SavedVoice> {
    store(&SavedVoice {
        name: name.trim().to_owned(),
        ..voice.clone()
    })
}

/// Keep `reading`'s stability and similarity as how `voice` reads.
pub fn set_voice_reading(voice: &SavedVoice, reading: &Reading) -> Result<SavedVoice> {
    store(&SavedVoice {
        stability: reading.stability,
        similarity: reading.similarity,
        ..voice.clone()
    })
}

/// Remember that `who` speaks with library voice `voice`, and with no
/// other.
pub fn link_speaker(lib: &Library, voice: LibraryId, who: &VoiceSpeaker) -> Result<()> {
    if lib.voice(voice).is_none() {
        return Err("that voice is not in the library".into());
    }
    for v in &lib.voices {
        let has = v.speakers.contains(who);
        if has == (v.id == voice) {
            continue;
        }
        let mut speakers = v.speakers.clone();
        speakers.retain(|s| s != who);
        if v.id == voice {
            speakers.push(who.clone());
        }
        store(&SavedVoice {
            speakers,
            ..v.clone()
        })?;
    }
    Ok(())
}

// -------------------------------------------------------------------- shared

/// Remove an item's file.
pub fn remove(path: &Path) -> Result<()> {
    std::fs::remove_file(path).map_err(|e| format!("can't remove {}: {e}", name(path)))
}

/// Copy a `.flikkfx` or `.flikkpreset` file into the library. An item with the
/// same id is replaced.
pub fn import(lib: &Library, path: &Path) -> Result<()> {
    match path.extension().and_then(|e| e.to_str()) {
        Some(PROGRAM_EXT) => {
            let program: Program = read(path)?;
            let mut asset = program.asset;
            asset.origin = lib.program(program.id).map(|p| p.id);
            match asset.origin {
                Some(_) => save_program(lib, &asset).map(|_| ()),
                None => {
                    asset.validate()?;
                    store(&Program {
                        id: program.id,
                        asset,
                        path: PathBuf::new(),
                    })
                    .map(|_| ())
                }
            }
        }
        Some(PRESET_EXT) => {
            let mut preset: Preset = read(path)?;
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
