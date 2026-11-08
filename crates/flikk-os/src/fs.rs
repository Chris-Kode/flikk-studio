//! Files written whole or not at all, and outputs that never replace a
//! file this operation did not create.
//!
//! Every write goes to a hidden temp file beside its target, unique per
//! write (`.<name>.<pid>-<n>.tmp`), and is renamed over the target once
//! complete: a reader sees the old file or the new one, never part of one,
//! and two writers of one file never share a temp file. The temp file is
//! removed when the write fails; one a crash leaves ends in `.tmp`, which
//! [`sweep_temps`] (and, under the cache, the cache sweep) removes.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

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

/// The process that made the temp file named `name` (a [`temp_sibling`]
/// name, `.<name>.<pid>-<n>.tmp`), or `None` when `name` is not one.
pub fn temp_owner(name: &str) -> Option<u32> {
    let tag = name.strip_prefix('.')?.strip_suffix(".tmp")?;
    let (pid, n) = tag.rsplit_once('.')?.1.split_once('-')?;
    n.parse::<u64>().ok()?;
    pid.parse().ok()
}

/// Remove the temp files ([`temp_sibling`]) a crash left directly in
/// `dir`, once last modified more than `age` ago: a younger one may be a
/// write under way in another process. Nothing else is touched, and a
/// missing folder is nothing to sweep.
pub fn sweep_temps(dir: &Path, age: Duration) {
    let now = SystemTime::now();
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let temp = entry
            .file_name()
            .to_str()
            .is_some_and(|n| temp_owner(n).is_some());
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|at| now.duration_since(at).is_ok_and(|a| a > age));
        if temp && old && entry.file_type().is_ok_and(|t| t.is_file()) {
            let _ = fs::remove_file(entry.path());
        }
    }
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
    let (tmp, mut file) = claim_temp(path, kind)?;
    let written = (|| {
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

/// A new temp file beside `path`, created by this call. A name a crash
/// left (its process id since reused) is passed over, never opened or
/// removed: it is not this write's.
fn claim_temp(path: &Path, kind: Kind) -> io::Result<(PathBuf, File)> {
    let mut tries = 0;
    loop {
        let tmp = temp_sibling(path);
        match open_new(&tmp, kind) {
            Ok(file) => return Ok((tmp, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists && tries < 8 => tries += 1,
            Err(e) => return Err(e),
        }
    }
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
