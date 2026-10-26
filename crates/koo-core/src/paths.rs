//! User data lives in `~/.koo-editor/`, or `$KOOEDITOR_HOME` when set.
//! Nothing is ever written next to sources or inside a repository.

use std::path::PathBuf;

pub fn home() -> PathBuf {
    if let Some(dir) = std::env::var_os("KOOEDITOR_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(dir);
    }
    let user = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .unwrap_or_else(|| ".".into());
    PathBuf::from(user).join(".koo-editor")
}

/// Created on first use.
fn sub(name: &str) -> PathBuf {
    let dir = home().join(name);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn database() -> PathBuf {
    let _ = std::fs::create_dir_all(home());
    home().join("koo-editor.db")
}

pub fn projects() -> PathBuf {
    sub("projects")
}

/// Proxies, collages, waveforms, analysis results; keyed by fingerprint.
pub fn cache() -> PathBuf {
    sub("cache")
}

/// Whisper and other bundled models.
pub fn models() -> PathBuf {
    sub("models")
}

/// Shared asset library (PRD M5).
pub fn assets() -> PathBuf {
    sub("assets")
}

pub fn logs() -> PathBuf {
    sub("logs")
}
