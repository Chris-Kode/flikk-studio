//! Linux ships a bare `flikk` binary. Without a desktop entry it is missing
//! from the app launcher, and a file manager asked to open it may start it
//! in a terminal. Register one for the running binary, so the launcher
//! starts the editor on its own.

use std::path::{Path, PathBuf};

/// Marks an entry this module wrote. One without it (the user's own, or a
/// distribution package's) is left alone.
const MARKER: &str = "X-Flikk-Generated=true";

/// Point the user's desktop entry at the running binary, writing it only
/// when it is missing or out of date. Debug builds register nothing, so
/// `cargo run` never replaces an installed copy's entry. Failures are
/// reported and otherwise ignored: the editor runs without an entry.
pub fn register() {
    if cfg!(debug_assertions) {
        return;
    }
    let Some(path) = entry_path() else {
        return;
    };
    let Some(contents) = std::env::current_exe().ok().and_then(|exe| entry(&exe)) else {
        return;
    };
    match std::fs::read_to_string(&path) {
        Ok(existing) if existing == contents || !existing.contains(MARKER) => return,
        _ => {}
    }
    if let Err(e) = flikk_os::fs::write_atomic(&path, contents.as_bytes()) {
        eprintln!("flikk: can't write {}: {e}", path.display());
    }
}

/// `$XDG_DATA_HOME/applications/`, by default under `~/.local/share`.
fn entry_path() -> Option<PathBuf> {
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| std::env::home_dir().map(|home| home.join(".local/share")))?;
    Some(data_home.join("applications/studio.flikk.Flikk.desktop"))
}

/// The entry for `exe`, or `None` for a path a desktop entry can't hold.
fn entry(exe: &Path) -> Option<String> {
    let exe = exe.to_str().filter(|path| !path.contains(['\n', '\r']))?;
    Some(format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Flikk\n\
         GenericName=Video Editor\n\
         Exec={} %f\n\
         Terminal=false\n\
         Categories=AudioVideo;Video;AudioVideoEditing;\n\
         {MARKER}\n",
        exec_argument(exe)
    ))
}

/// `path` quoted as an `Exec` argument. The desktop entry spec escapes
/// twice: inside the quotes for the command line, then the whole value as
/// a string, which doubles every backslash.
fn exec_argument(path: &str) -> String {
    let mut quoted = String::from("\"");
    for c in path.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    quoted.replace('\\', "\\\\")
}
