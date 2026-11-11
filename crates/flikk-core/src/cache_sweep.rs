//! What is made again when missing goes once nothing has used it for a
//! while: [`sweep`] runs at startup, off the UI thread. A file's modified
//! time is when it was last used: readers call [`mark_used`].
//!
//! The render cache and optical flow frames are not swept: they keep to
//! their own size limits ([`crate::disk_cache`]). Mattes and analysis
//! results are left too, as they cost a model run or a provider to make
//! again.

use crate::paths;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const DAY: Duration = Duration::from_secs(24 * 60 * 60);
/// Unused this long, a file that can be made again goes.
pub const UNUSED: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Kept until unused for [`UNUSED`].
const KEPT: [&str; 9] = [
    "proxies",
    "depth",
    "collages",
    "waveforms",
    "frames",
    "agent",
    "voices",
    "voice-previews",
    "hdr",
];

/// Only needed while a request is out: what a provider or ffmpeg was
/// handed, and renders' scratch folders. A day old, they are leftovers.
const SCRATCH: [&str; 4] = ["voice-sources", "upload", "respeak", "render"];

/// Disk caches that keep to a size limit and remove their own leftovers.
const OWN_SWEEP: [&str; 2] = ["frame-cache", "interpolated"];

/// Mark `path` as used now, so [`sweep`] keeps it. Cheap when it was
/// marked within the day.
pub fn mark_used(path: &Path) {
    let now = SystemTime::now();
    let recent = fs::metadata(path)
        .and_then(|m| m.modified())
        .is_ok_and(|t| now.duration_since(t).is_ok_and(|age| age < DAY));
    if !recent && let Ok(file) = fs::File::options().write(true).open(path) {
        let _ = file.set_modified(now);
    }
}

/// Remove what nothing has used for long, scratch files left behind,
/// proxies made under a tone mapping no longer used, and what writes to
/// the user's data a crash left.
pub fn sweep() {
    let cache = paths::cache();
    let now = SystemTime::now();
    for dir in KEPT.into_iter().chain(crate::cleanup::FOLDERS) {
        older_than(&cache.join(dir), now, UNUSED);
    }
    for dir in SCRATCH {
        older_than(&cache.join(dir), now, DAY);
    }
    // HTML pages; the browser's profiles are swept by the browser.
    for file in read_dir(&cache.join("html")) {
        if file.is_file() && age(&file, now).is_some_and(|a| a > UNUSED) {
            let _ = fs::remove_file(file);
        }
    }
    leftovers(&cache, now);
    superseded_proxies(&cache.join("proxies"));
    library_leftovers(&paths::library(), now);
    // Settings, credentials, the agent's stores, chats and proposals,
    // autosaves and backups are written whole through a temp file.
    let home = paths::home();
    for dir in [
        home.clone(),
        home.join("agent"),
        paths::projects(),
        paths::autosave(),
        paths::backups(),
    ] {
        flikk_os::fs::sweep_temps(&dir, DAY);
    }
}

/// Remove the files under `dir` last modified more than `max` ago, and
/// the folders left empty.
fn older_than(dir: &Path, now: SystemTime, max: Duration) {
    for entry in read_dir(dir) {
        if entry.is_dir() {
            older_than(&entry, now, max);
            let _ = fs::remove_dir(&entry);
        } else if age(&entry, now).is_some_and(|a| a > max) {
            let _ = fs::remove_file(&entry);
        }
    }
}

/// Partial writes of anything under the cache, a day old: `.part`,
/// `.tmp` and `.tmp-…` files.
fn leftovers(dir: &Path, now: SystemTime) {
    for entry in read_dir(dir) {
        if entry.is_dir() {
            // Browser profiles hold files of their own naming; the disk
            // caches remove their own.
            let name = entry
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if name.starts_with("profile-") || OWN_SWEEP.contains(&name.as_str()) {
                continue;
            }
            leftovers(&entry, now);
            continue;
        }
        let name = entry
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let partial = name.ends_with(".part")
            || name.ends_with(".tmp")
            || name
                .rsplit('.')
                .next()
                .is_some_and(|e| e.starts_with("tmp-"));
        if partial && age(&entry, now).is_some_and(|a| a > DAY) {
            let _ = fs::remove_file(&entry);
        }
    }
}

/// What library writes a crash left, a day old: hidden `.tmp` files under
/// the library and its assets, and the empty files that claim a new item's
/// name until it is written. Nothing else there is touched: the library
/// is the user's data.
fn library_leftovers(dir: &Path, now: SystemTime) {
    use crate::library::{PRESET_EXT, PROGRAM_EXT, SOUND_EXT, VOICE_EXT};
    for entry in read_dir(dir) {
        if entry.is_dir() {
            library_leftovers(&entry, now);
            continue;
        }
        let temp = entry
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| flikk_os::fs::temp_owner(n).is_some());
        let claim = entry
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| [PROGRAM_EXT, PRESET_EXT, SOUND_EXT, VOICE_EXT].contains(&e))
            && fs::metadata(&entry).is_ok_and(|m| m.len() == 0);
        if (temp || claim) && age(&entry, now).is_some_and(|a| a > DAY) {
            let _ = fs::remove_file(&entry);
        }
    }
}

/// Proxies are `<fingerprint>.mp4`, or `<fingerprint>-<mapping>.mp4` for
/// HDR media tone mapped: of one fingerprint's, those unused for a day
/// longer than the newest were made for a mapping since changed.
fn superseded_proxies(dir: &Path) {
    let mut by_source: std::collections::HashMap<String, Vec<(SystemTime, PathBuf)>> =
        Default::default();
    for file in read_dir(dir) {
        let Some(stem) = file.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
            continue;
        };
        if file.extension().is_none_or(|e| e != "mp4") {
            continue;
        }
        let Ok(used) = fs::metadata(&file).and_then(|m| m.modified()) else {
            continue;
        };
        let fingerprint = stem.split('-').next().unwrap_or(&stem).to_owned();
        by_source.entry(fingerprint).or_default().push((used, file));
    }
    for files in by_source.values().filter(|f| f.len() > 1) {
        let Some(newest) = files.iter().map(|(t, _)| *t).max() else {
            continue;
        };
        for (used, file) in files {
            if newest.duration_since(*used).is_ok_and(|d| d > DAY) {
                let _ = fs::remove_file(file);
            }
        }
    }
}

fn age(path: &Path, now: SystemTime) -> Option<Duration> {
    let modified = fs::metadata(path).and_then(|m| m.modified()).ok()?;
    now.duration_since(modified).ok()
}

fn read_dir(dir: &Path) -> impl Iterator<Item = PathBuf> {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
}
