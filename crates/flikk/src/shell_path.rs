//! Opened from Finder, the Dock or a desktop launcher, the editor gets a
//! bare PATH (`/usr/bin:/bin:/usr/sbin:/sbin` on macOS) without the
//! folders the agent CLIs live in, such as `/opt/homebrew/bin`. Read the
//! PATH the user's login shell builds and run with that instead.

use std::ffi::OsString;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const MARKER: &str = "__FLIKK_PATH__";
const TIMEOUT: Duration = Duration::from_secs(5);

/// Call before any other thread starts: it sets PATH.
pub fn import() {
    // A terminal already handed over the shell's PATH.
    if std::env::var_os("TERM").is_some() {
        return;
    }
    let current = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut add = |dir: PathBuf| {
        if !dir.as_os_str().is_empty() && !dirs.contains(&dir) {
            dirs.push(dir);
        }
    };
    if let Some(shell_path) = login_shell_path() {
        std::env::split_paths(&shell_path).for_each(&mut add);
    }
    std::env::split_paths(&current).for_each(&mut add);
    // Where the CLIs usually install, for a shell that couldn't be read.
    let home = flikk_core::paths::home();
    for dir in [
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        home.join(".local/bin"),
        home.join(".claude/local"),
    ] {
        if dir.is_dir() {
            add(dir);
        }
    }
    let Ok(path) = std::env::join_paths(dirs) else {
        return;
    };
    if path != current {
        // SAFETY: called from `main` before any other thread exists.
        unsafe { std::env::set_var("PATH", path) };
    }
}

/// `$SHELL`'s PATH as an interactive login shell sets it, so both
/// `.zprofile` and `.zshrc` (or their bash and fish counterparts) count.
fn login_shell_path() -> Option<OsString> {
    let shell = std::env::var_os("SHELL")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/bin/sh".into());
    let mut child = Command::new(shell)
        .args(["-i", "-l", "-c"])
        .arg(format!("printf '{MARKER}%s{MARKER}' \"$PATH\""))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < TIMEOUT => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    // Startup files may print banners around it.
    let (_, rest) = out.split_once(MARKER)?;
    let (path, _) = rest.split_once(MARKER)?;
    (!path.is_empty()).then(|| path.into())
}
