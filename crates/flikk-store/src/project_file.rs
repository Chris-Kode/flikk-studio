//! `.flikk` project files: the project as JSON, wherever the user saves it.
//!
//! Files are written atomically (a temporary file beside the target,
//! flushed, then renamed over it), so a crash mid-save leaves the last good
//! version. Media keeps its absolute path plus its path relative to the
//! file; opening tries the absolute path, then the relative one, so a folder
//! holding a project and its footage can move or change machines.
//!
//! The same format holds autosaves (`paths::autosave()`, one per project id,
//! naming the file they belong to) and backups (`paths::backups()`).

use anyhow::{Context, Result, bail};
use flikk_core::ids::{MediaId, ProjectId};
use flikk_core::model::Project;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

pub const EXTENSION: &str = "flikk";
const FORMAT: &str = "flikk-project";
/// Bumped when an older file needs an upgrade step on open.
const VERSION: u32 = 1;

/// A saved file gets a backup copy when the newest is older than this.
pub const BACKUP_EVERY: Duration = Duration::from_secs(5 * 60);
/// Backups kept per project; older ones are deleted.
pub const BACKUPS_KEPT: usize = 10;

#[derive(Serialize)]
struct FileOut<'a> {
    format: &'static str,
    version: u32,
    project: &'a Project,
    /// Media paths relative to the project file, by media id.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    relative_paths: BTreeMap<MediaId, PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    autosave_of: Option<&'a Path>,
}

#[derive(Deserialize)]
struct FileIn {
    format: String,
    version: u32,
    project: Project,
    #[serde(default)]
    relative_paths: BTreeMap<MediaId, PathBuf>,
    #[serde(default)]
    autosave_of: Option<PathBuf>,
}

/// A project as read from a file.
#[derive(Clone, Debug)]
pub struct ProjectFile {
    pub project: Project,
    /// For an autosave: the file whose unsaved changes it holds, `None` for
    /// a project never saved.
    pub autosave_of: Option<PathBuf>,
}

/// Read a project file or an autosave. Media found only through its
/// relative path is given that path.
pub fn read(path: &Path) -> Result<ProjectFile> {
    let bytes = std::fs::read(path).with_context(|| format!("can't read {}", path.display()))?;
    let file: FileIn = serde_json::from_slice(&bytes)
        .map_err(|e| anyhow::anyhow!("{} is not a Flikk project ({e})", file_name(path)))?;
    if file.format != FORMAT {
        bail!("{} is not a Flikk project", file_name(path));
    }
    if file.version > VERSION {
        bail!(
            "{} was saved by a newer Flikk; update to open it",
            file_name(path)
        );
    }
    let mut project = file.project;
    let base = file.autosave_of.as_deref().unwrap_or(path).parent();
    if let Some(base) = base {
        for item in &mut project.media {
            if item.path.exists() {
                continue;
            }
            let Some(rel) = file.relative_paths.get(&item.id) else {
                continue;
            };
            let found = base.join(rel);
            if found.exists() {
                item.path = std::fs::canonicalize(&found).unwrap_or(found);
            }
        }
    }
    Ok(ProjectFile {
        project,
        autosave_of: file.autosave_of,
    })
}

/// Save `project` to `path`.
pub fn write(path: &Path, project: &Project) -> Result<()> {
    write_file(path, path, project, None).with_context(|| format!("can't save {}", path.display()))
}

/// `relative_to` is the file whose folder relative media paths start from.
pub(crate) fn write_file(
    path: &Path,
    relative_to: &Path,
    project: &Project,
    autosave_of: Option<&Path>,
) -> Result<()> {
    let relative_paths = relative_to
        .parent()
        .map(|dir| {
            project
                .media
                .iter()
                .filter_map(|m| Some((m.id, relative(dir, &m.path)?)))
                .collect()
        })
        .unwrap_or_default();
    let out = FileOut {
        format: FORMAT,
        version: VERSION,
        project,
        relative_paths,
        autosave_of,
    };
    write_atomic(path, &serde_json::to_vec_pretty(&out)?)
}

/// Write beside the target, flush, then rename over it.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty());
    if let Some(dir) = dir {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_file_name(format!(".{}.tmp", file_name(path)));
    let written = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    Ok(written?)
}

/// `path` from `dir` (`../Footage/a.mov`); `None` across drives or for
/// relative input.
fn relative(dir: &Path, path: &Path) -> Option<PathBuf> {
    if !dir.is_absolute() || !path.is_absolute() {
        return None;
    }
    let mut from = dir.components().peekable();
    let mut to = path.components().peekable();
    while let (Some(a), Some(b)) = (from.peek(), to.peek()) {
        if a != b {
            break;
        }
        from.next();
        to.next();
    }
    let mut out = PathBuf::new();
    for c in from {
        match c {
            Component::Normal(_) => out.push(".."),
            Component::CurDir => {}
            // A different root or drive.
            _ => return None,
        }
    }
    for c in to {
        match c {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            _ => return None,
        }
    }
    Some(out)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// The project name for a file: its name without the extension.
pub fn name_of(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled".into())
}

// ------------------------------------------------------------------ autosave

/// Where a project's unsaved changes are kept.
pub fn autosave_path(id: ProjectId) -> PathBuf {
    flikk_core::paths::autosave().join(format!("{}.{EXTENSION}", id.0.simple()))
}

/// Autosaves left by sessions that did not close cleanly, newest first.
pub fn autosaves() -> Vec<PathBuf> {
    let mut found: Vec<(SystemTime, PathBuf)> = std::fs::read_dir(flikk_core::paths::autosave())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == EXTENSION))
        .filter_map(|p| Some((modified(&p)?, p)))
        .collect();
    found.sort_by_key(|(t, _)| std::cmp::Reverse(*t));
    found.into_iter().map(|(_, p)| p).collect()
}

/// Forget a project's unsaved changes (saved, or discarded by the user).
pub fn remove_autosave(id: ProjectId) {
    let _ = std::fs::remove_file(autosave_path(id));
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

// ------------------------------------------------------------------- backups

/// Copy a just-saved file into its backup folder when the newest backup is
/// older than [`BACKUP_EVERY`], keeping the newest [`BACKUPS_KEPT`].
pub fn backup(path: &Path, id: ProjectId) -> Result<()> {
    let name = name_of(path);
    let dir = flikk_core::paths::backups().join(format!("{} {id}", sanitize(&name)));
    std::fs::create_dir_all(&dir)?;
    let mut kept: Vec<(SystemTime, PathBuf)> = std::fs::read_dir(&dir)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == EXTENSION))
        .filter_map(|p| Some((modified(&p)?, p)))
        .collect();
    kept.sort_by_key(|(t, _)| std::cmp::Reverse(*t));
    let recent = kept
        .first()
        .is_some_and(|(t, _)| t.elapsed().map_or(true, |age| age < BACKUP_EVERY));
    if !recent {
        let stamp = chrono::Local::now().format("%Y-%m-%d %H-%M-%S");
        let copy = dir.join(format!("{} {stamp}.{EXTENSION}", sanitize(&name)));
        std::fs::copy(path, &copy)?;
        kept.insert(0, (SystemTime::now(), copy));
    }
    for (_, old) in kept.iter().skip(BACKUPS_KEPT) {
        let _ = std::fs::remove_file(old);
    }
    Ok(())
}

/// A file name part: no path separators or characters Windows refuses.
pub fn sanitize(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let clean = clean.trim().trim_matches('.').to_owned();
    if clean.is_empty() {
        "Untitled".into()
    } else {
        clean
    }
}
