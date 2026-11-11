//! Whether a backend's CLI is installed, and which version: what the
//! welcome page's setup list shows. The CLI is found as a turn finds it
//! (`FLIKK_CLAUDE`, `FLIKK_CODEX`, else `PATH`) and asked for
//! `--version`.

use super::{isolate, locate};
use flikk_core::agent::Backend;
use flikk_os::process::ChildGuard;
use std::io::Read;
use std::process::Stdio;
use std::sync::mpsc;
use std::time::Duration;

/// How long a CLI gets to print its version.
const TIMEOUT: Duration = Duration::from_secs(10);

/// The version `backend`'s CLI reports ("2.0.14"), or `None` when it is
/// not found or does not answer in time. Blocks for up to [`TIMEOUT`]; run
/// it off the UI thread.
pub fn installed_version(backend: Backend) -> Option<String> {
    let (name, var) = match backend {
        Backend::Claude => ("claude", "FLIKK_CLAUDE"),
        Backend::Codex => ("codex", "FLIKK_CODEX"),
    };
    let mut command = locate(name, var).ok()?;
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    isolate(&mut command);
    let mut child = ChildGuard::spawn(&mut command).ok()?;
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut out = String::new();
        let _ = stdout.read_to_string(&mut out);
        let _ = tx.send(out);
    });
    // Killed on drop when it does not answer in time.
    let printed = rx.recv_timeout(TIMEOUT).ok();
    drop(child);
    version_in(&printed?)
}

/// The first word that starts with a digit: "2.0.14" in "2.0.14 (Claude
/// Code)", "0.46.0" in "codex-cli 0.46.0".
fn version_in(printed: &str) -> Option<String> {
    printed
        .split_whitespace()
        .map(|word| word.trim_start_matches('v'))
        .find(|word| word.starts_with(|c: char| c.is_ascii_digit()))
        .map(str::to_owned)
}
