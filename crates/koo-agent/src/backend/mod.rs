//! Backend adapters: how to launch Claude Code or Codex for one turn and how
//! to read its event stream. Each turn is one subprocess on a background
//! thread; the conversation continues through the backend's own session id.

mod claude;
mod codex;

pub use claude::Claude;
pub use codex::Codex;

use koo_core::agent::Backend as BackendKind;
use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

/// How the backend should start `koo-mcp`.
#[derive(Clone, Debug)]
pub struct McpLaunch {
    pub command: PathBuf,
    pub env: Vec<(String, String)>,
}

/// Everything a backend needs for one turn.
#[derive(Clone, Debug)]
pub struct TurnSpec {
    pub system_prompt: String,
    /// User message with the context block.
    pub message: String,
    /// Backend session to continue, if any.
    pub resume: Option<String>,
    pub mcp: McpLaunch,
    /// Empty working directory under `paths::home()`, so no project file of
    /// the user's (CLAUDE.md, AGENTS.md, .codex) is ever picked up.
    pub workdir: PathBuf,
    /// Scratch directory for this turn (MCP config file).
    pub run_dir: PathBuf,
}

/// What the adapters read from the CLI output.
#[derive(Clone, Debug, PartialEq)]
pub enum BackendEvent {
    /// The backend's session id; persist it to resume the chat.
    Session(String),
    TextDelta(String),
    /// The turn is over. `error` is set when it failed.
    Finished {
        error: Option<String>,
    },
}

pub trait Adapter: Send {
    fn kind(&self) -> BackendKind;
    /// Build the command line. May write files under `turn.run_dir`.
    fn command(&self, turn: &TurnSpec) -> std::io::Result<Command>;
    /// Written to stdin once the process starts.
    fn stdin(&self, turn: &TurnSpec) -> String;
    /// Keep stdin open until the turn finishes (streaming input).
    fn hold_stdin(&self) -> bool;
    fn parse(&mut self, line: &str) -> Vec<BackendEvent>;
}

pub fn adapter(kind: BackendKind) -> Box<dyn Adapter> {
    match kind {
        BackendKind::Claude => Box::new(Claude::default()),
        BackendKind::Codex => Box::new(Codex::default()),
    }
}

/// A running turn. Dropping it does not stop the process; call [`kill`].
///
/// [`kill`]: RunningTurn::kill
pub struct RunningTurn {
    child: Arc<Mutex<Option<Child>>>,
    pub events: Receiver<BackendEvent>,
}

impl RunningTurn {
    pub fn kill(&self) {
        if let Some(child) = self.child.lock().unwrap().as_mut() {
            let _ = child.kill();
        }
    }
}

/// Start the backend for `turn` on background threads.
pub fn start(mut adapter: Box<dyn Adapter>, turn: TurnSpec) -> std::io::Result<RunningTurn> {
    let mut command = adapter.command(&turn)?;
    command
        .current_dir(&turn.workdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // When the editor itself runs under Claude Code, don't let the child
    // think it is a nested session.
    for var in [
        "CLAUDECODE",
        "CLAUDE_CODE_SESSION_ID",
        "CLAUDE_CODE_ENTRYPOINT",
    ] {
        command.env_remove(var);
    }
    hide_window(&mut command);
    let mut child = command.spawn()?;
    let mut stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    if let Some(w) = stdin.as_mut() {
        let payload = adapter.stdin(&turn);
        let _ = w.write_all(payload.as_bytes()).and_then(|_| w.flush());
    }
    if !adapter.hold_stdin() {
        drop(stdin.take());
    }
    let child = Arc::new(Mutex::new(Some(child)));
    let (tx, rx) = mpsc::channel();

    // Keep the tail of stderr for error messages.
    let tail = Arc::new(Mutex::new(VecDeque::<String>::new()));
    if let Some(mut err) = stderr {
        let tail = tail.clone();
        thread::spawn(move || {
            let mut buf = String::new();
            let _ = err.read_to_string(&mut buf);
            let mut t = tail.lock().unwrap();
            for line in buf.lines().filter(|l| !l.trim().is_empty()) {
                t.push_back(line.to_owned());
                if t.len() > 8 {
                    t.pop_front();
                }
            }
        });
    }

    let waiter = child.clone();
    thread::Builder::new()
        .name("koo-agent-backend".into())
        .spawn(move || {
            let finished = read_events(&mut *adapter, stdout, &mut stdin, &tx);
            drop(stdin);
            // Poll so `kill` can take the lock meanwhile.
            let status = loop {
                match waiter.lock().unwrap().as_mut().map(|c| c.try_wait()) {
                    Some(Ok(None)) => {}
                    Some(Ok(Some(s))) => break Some(Ok(s)),
                    Some(Err(e)) => break Some(Err(e)),
                    None => break None,
                }
                thread::sleep(std::time::Duration::from_millis(25));
            };
            if !finished {
                // Give the stderr reader a moment to finish.
                thread::sleep(std::time::Duration::from_millis(50));
                let tail: Vec<_> = tail.lock().unwrap().iter().cloned().collect();
                let error = match status {
                    Some(Ok(s)) if s.success() => None,
                    Some(Ok(s)) => Some(if tail.is_empty() {
                        format!("{} exited with {s}", adapter.kind().label())
                    } else {
                        tail.join("\n")
                    }),
                    Some(Err(e)) => Some(e.to_string()),
                    None => None,
                };
                let _ = tx.send(BackendEvent::Finished {
                    error: error.or(Some("The agent stopped before finishing".into())),
                });
            }
            waiter.lock().unwrap().take();
        })?;
    Ok(RunningTurn { child, events: rx })
}

/// Pump stdout through the adapter. Returns whether `Finished` was sent.
fn read_events(
    adapter: &mut dyn Adapter,
    stdout: Option<std::process::ChildStdout>,
    stdin: &mut Option<std::process::ChildStdin>,
    tx: &Sender<BackendEvent>,
) -> bool {
    let Some(stdout) = stdout else { return false };
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        for event in adapter.parse(&line) {
            let done = matches!(event, BackendEvent::Finished { .. });
            if tx.send(event).is_err() {
                return true;
            }
            if done {
                // Closing stdin lets a streaming-input CLI exit.
                drop(stdin.take());
                return true;
            }
        }
    }
    false
}

#[cfg(windows)]
fn hide_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_window(_: &mut Command) {}

/// Find a CLI: `$override_var`, else `name` on `PATH` (with `PATHEXT` on
/// Windows). Batch shims (`claude.cmd`) run through `cmd /C`.
pub fn locate(name: &str, override_var: &str) -> std::io::Result<Command> {
    let path = std::env::var_os(override_var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| search_path(name))
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("`{name}` was not found on PATH (set {override_var} to its location)"),
            )
        })?;
    let is_batch = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"));
    Ok(if is_batch {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(path);
        c
    } else {
        Command::new(path)
    })
}

fn search_path(name: &str) -> Option<PathBuf> {
    let exts: Vec<OsString> = if cfg!(windows) {
        std::env::var_os("PATHEXT")
            .map(|v| {
                v.to_string_lossy()
                    .split(';')
                    .filter(|e| !e.is_empty())
                    .map(OsString::from)
                    .collect()
            })
            .unwrap_or_else(|| vec![".EXE".into(), ".CMD".into(), ".BAT".into()])
    } else {
        vec![OsString::new()]
    };
    let dirs = std::env::var_os("PATH")?;
    std::env::split_paths(&dirs).find_map(|dir| {
        exts.iter().find_map(|ext| {
            let mut file = OsString::from(name);
            file.push(ext);
            let candidate = dir.join(file);
            is_file(&candidate).then_some(candidate)
        })
    })
}

fn is_file(p: &Path) -> bool {
    p.metadata().map(|m| m.is_file()).unwrap_or(false)
}

/// `koo-mcp` next to the running executable, or `$KOOEDITOR_MCP`.
pub fn mcp_binary() -> std::io::Result<PathBuf> {
    if let Some(p) = std::env::var_os("KOOEDITOR_MCP").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(p));
    }
    let exe = std::env::current_exe()?;
    let name = format!("koo-mcp{}", std::env::consts::EXE_SUFFIX);
    let candidates = [
        exe.parent().map(|d| d.join(&name)),
        // `target/debug/deps` when run from cargo tooling.
        exe.parent().and_then(|d| d.parent()).map(|d| d.join(&name)),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|p| is_file(p))
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{name} was not found next to the editor (set KOOEDITOR_MCP)"),
            )
        })
}

/// Joins text pieces that arrive as separate messages with a blank line.
#[derive(Default)]
pub(crate) struct TextJoin {
    any: bool,
    pending_break: bool,
}

impl TextJoin {
    /// A new assistant message starts; the next text gets a separator.
    pub fn new_message(&mut self) {
        if self.any {
            self.pending_break = true;
        }
    }

    pub fn text(&mut self, text: &str) -> Option<BackendEvent> {
        if text.is_empty() {
            return None;
        }
        let mut out = String::new();
        if self.pending_break {
            out.push_str("\n\n");
            self.pending_break = false;
        }
        out.push_str(text);
        self.any = true;
        Some(BackendEvent::TextDelta(out))
    }
}
