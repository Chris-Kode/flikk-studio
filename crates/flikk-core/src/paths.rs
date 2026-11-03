//! User data lives in `~/.flikk/`, or `$FLIKK_HOME` when set.
//! Nothing is ever written next to sources or inside a repository; the only
//! files written elsewhere are the ones the user saves there (`.flikk`
//! projects, exports).

use std::path::PathBuf;

pub fn home() -> PathBuf {
    if let Some(dir) = std::env::var_os("FLIKK_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(dir);
    }
    let user = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .unwrap_or_else(|| ".".into());
    PathBuf::from(user).join(".flikk")
}

/// Created on first use.
fn sub(name: &str) -> PathBuf {
    let dir = home().join(name);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn database() -> PathBuf {
    let _ = std::fs::create_dir_all(home());
    home().join("flikk.db")
}

/// Per-project data that stays with the user, not the `.flikk` file (chats),
/// keyed by project id.
pub fn projects() -> PathBuf {
    sub("projects")
}

/// Unsaved changes of open projects, for crash recovery.
pub fn autosave() -> PathBuf {
    sub("autosave")
}

/// Timed copies of saved projects.
pub fn backups() -> PathBuf {
    sub("backups")
}

/// FX programs, presets and sounds shared by every project.
pub fn library() -> PathBuf {
    sub("library")
}

/// Proxies, collages, waveforms, analysis results; keyed by fingerprint.
pub fn cache() -> PathBuf {
    sub("cache")
}

/// Whisper and other bundled models.
pub fn models() -> PathBuf {
    sub("models")
}

/// Audio of the library's sounds, in a folder per content.
pub fn assets() -> PathBuf {
    sub("assets")
}

/// Music, sound effects, voiceovers and videos providers made, each with a `.json`
/// of how, and pictures Codex made.
pub fn generated() -> PathBuf {
    sub("generated")
}

/// Pictures the agent took from the web for the pool.
pub fn web() -> PathBuf {
    sub("web")
}

/// API keys, owner-only, where the system has no credential store.
pub fn credentials() -> PathBuf {
    let _ = std::fs::create_dir_all(home());
    home().join("credentials.json")
}

pub fn logs() -> PathBuf {
    sub("logs")
}
