//! Files written whole or not at all, and outputs that never replace a
//! file this operation did not create.
//!
//! Every write goes to a hidden temp file beside its target, unique per
//! write (`.<name>.<pid>-<n>.tmp`), and is renamed over the target once
//! complete: a reader sees the old file or the new one, never part of one,
//! and two writers of one file never share a temp file. The temp file is
//! removed when the write fails; one a crash leaves ends in `.tmp`, which
//! the cache sweep removes.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Write `bytes` to `path` whole or not at all, creating its folder. Not
/// synced to disk: for caches and other data that can be made again.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic_with(path, |file| file.write_all(bytes))
}

/// Like [`write_atomic`], with the contents written by `write` into the
/// temp file: for encoders that stream into a file.
pub fn write_atomic_with(
    path: &Path,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<()> {
    replace(path, Kind::Cache, write)
}

/// Write `bytes` to `path` whole or not at all, creating its folder, and
/// make the file and its name durable before returning: for the user's
/// data (projects, settings, sessions), which must survive a power cut.
pub fn write_durable(path: &Path, bytes: &[u8]) -> io::Result<()> {
    replace(path, Kind::Durable, |file| file.write_all(bytes))
}

/// Like [`write_durable`], readable and writable by its owner only on Unix:
/// for secrets such as API keys and the agent bridge's token.
pub fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    replace(path, Kind::Private, |file| file.write_all(bytes))
}

/// A hidden name beside `path` for a temp file, unique per call:
/// `.<name>.<pid>-<n>.tmp`.
pub fn temp_sibling(path: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!(
        ".{name}.{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Move the finished file `from` to `to` unless a file is already there.
/// `Ok(false)` when one is: nothing is replaced and `from` stays. Never
/// replaces a file, also on file systems without hard links, where the name
/// is claimed by an exclusive create first.
pub fn place_new(from: &Path, to: &Path) -> io::Result<bool> {
    match fs::hard_link(from, to) {
        Ok(()) => {
            let _ = fs::remove_file(from);
            return Ok(true);
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
        // File systems without hard links (FAT, exFAT, some network shares).
        Err(_) => {}
    }
    // Claim the name with an exclusive create, then rename over the claim:
    // only our own empty file is ever replaced.
    match OpenOptions::new().write(true).create_new(true).open(to) {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
        Err(e) => return Err(e),
    }
    fs::rename(from, to).inspect_err(|_| {
        if fs::metadata(to).is_ok_and(|m| m.len() == 0) {
            let _ = fs::remove_file(to);
        }
    })?;
    Ok(true)
}

/// How a replacing write treats the new file.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Cache,
    Durable,
    Private,
}

fn replace(
    path: &Path,
    kind: Kind,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<()> {
    let folder = path.parent().filter(|d| !d.as_os_str().is_empty());
    if let Some(folder) = folder {
        fs::create_dir_all(folder)?;
    }
    let tmp = temp_sibling(path);
    let written = (|| {
        let mut file = open_new(&tmp, kind)?;
        write(&mut file)?;
        if kind != Kind::Cache {
            file.sync_all()?;
        }
        drop(file);
        fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written?;
    // Make the rename itself durable. Best effort: not every file system
    // lets a folder be synced.
    #[cfg(unix)]
    if kind != Kind::Cache
        && let Some(folder) = folder
    {
        let _ = File::open(folder).and_then(|d| d.sync_all());
    }
    Ok(())
}

fn open_new(path: &Path, kind: Kind) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if kind == Kind::Private {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = kind;
    options.open(path)
}
