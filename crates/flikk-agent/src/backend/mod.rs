//! Backend adapters: how to launch Claude Code or Codex for one turn and how
//! to read its event stream. Each turn is one subprocess on a background
//! thread; the conversation continues through the backend's own session id.

mod claude;
mod codex;
mod models;

pub(crate) use claude::Claude;
pub(crate) use codex::{Codex, codex_home};
pub use models::{cached_models, refresh_models};

use flikk_core::agent::{Backend as BackendKind, ModelChoice};
use flikk_os::process::{ChildGuard, command as new_command};
use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// How long the backend lets a `flikk` tool call run. A call that needs the
/// user's approval stays open until they decide, and a generated voiceover
/// takes a while after that.
pub(crate) const TOOL_TIMEOUT_SECS: u64 = 60 * 60;

/// How long a CLI gets to exit by itself once it reported the turn over.
const EXIT_GRACE: Duration = Duration::from_secs(10);

/// How the backend should start `flikk-mcp`.
#[derive(Clone, Debug)]
pub(crate) struct McpLaunch {
    pub command: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// Everything a backend needs for one turn.
#[derive(Clone, Debug)]
pub(crate) struct TurnSpec {
    pub system_prompt: String,
    /// User message with the context block.
    pub message: String,
    /// Backend session to continue, if any.
    pub resume: Option<String>,
    /// Model and thinking level; `None` fields keep the CLI default.
    pub model: ModelChoice,
    /// The editor's tools; `None` for a turn that only answers in text.
    pub mcp: Option<McpLaunch>,
    /// Let the backend search the web and read pages with its own tools.
    pub web: bool,
    /// Empty working directory under `paths::home()`, so no project file of
    /// the user's (CLAUDE.md, AGENTS.md, .codex) is ever picked up.
    pub workdir: PathBuf,
    /// Scratch directory for this turn (MCP config file).
    pub run_dir: PathBuf,
}

/// What the adapters read from the CLI output.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum BackendEvent {
    /// The backend's session id; persist it to resume the chat.
    Session(String),
    TextDelta(String),
    /// One of the backend's own tools began; a [`BackendEvent::Tool`] with
    /// the same `key` follows when it ends.
    ToolStarted {
        key: String,
        tool: &'static str,
        args: serde_json::Value,
        summary: String,
    },
    /// One of the backend's own tools ran, named the same for every
    /// backend. The `flikk` tools are reported where they are served. `key`
    /// is that of its [`BackendEvent::ToolStarted`], if there was one.
    Tool {
        key: Option<String>,
        tool: &'static str,
        args: serde_json::Value,
        summary: String,
        ok: bool,
    },
    /// How many tool calls, `flikk`'s included, the backend has begun and
    /// not yet recorded a result for; sent when it changes. Killed while
    /// one is open, the backend's session is left without that result.
    OpenCalls(usize),
    /// The turn is over. `error` is set when it failed.
    Finished {
        error: Option<String>,
    },
}

pub(crate) trait Adapter: Send {
    fn kind(&self) -> BackendKind;
    /// Build the command line. May write files under `turn.run_dir`.
    fn command(&self, turn: &TurnSpec) -> std::io::Result<Command>;
    /// Written to stdin once the process starts.
    fn stdin(&self, turn: &TurnSpec) -> String;
    /// Keep stdin open until the turn finishes (streaming input).
    fn hold_stdin(&self) -> bool;
    fn parse(&mut self, line: &str) -> Vec<BackendEvent>;
    /// The last error the CLI reported in its stream, for a stream that
    /// ends without [`BackendEvent::Finished`].
    fn last_error(&mut self) -> Option<String> {
        None
    }
}

pub(crate) fn adapter(kind: BackendKind) -> Box<dyn Adapter> {
    match kind {
        BackendKind::Claude => Box::new(Claude::default()),
        BackendKind::Codex => Box::new(Codex::default()),
    }
}

/// A running turn. Dropping it does not stop the process while its reader
/// thread runs; call [`kill`].
///
/// [`kill`]: RunningTurn::kill
pub(crate) struct RunningTurn {
    child: Arc<Mutex<Option<ChildGuard>>>,
    pub events: Receiver<BackendEvent>,
}

impl RunningTurn {
    pub(crate) fn kill(&self) {
        if let Some(child) = self.child.lock().unwrap().as_mut() {
            let _ = child.kill();
        }
    }
}

/// Start the backend for `turn` on background threads.
pub(crate) fn start(mut adapter: Box<dyn Adapter>, turn: TurnSpec) -> std::io::Result<RunningTurn> {
    let mut command = adapter.command(&turn)?;
    command
        .current_dir(&turn.workdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    isolate(&mut command);
    let mut child = ChildGuard::spawn(&mut command)?;
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
        .name("flikk-agent-backend".into())
        .spawn(move || {
            let finished = read_events(&mut *adapter, stdout, &mut stdin, &tx);
            drop(stdin);
            // Poll so `kill` can take the lock meanwhile. A CLI that said
            // the turn is over but doesn't exit is killed after a grace.
            let grace = Instant::now() + EXIT_GRACE;
            let status = loop {
                let polled = {
                    let mut guard = waiter.lock().unwrap();
                    // Only this thread takes the child, once it exited.
                    let child = guard.as_mut().expect("taken after the loop");
                    if finished && Instant::now() >= grace {
                        let _ = child.kill();
                    }
                    child.try_wait()
                };
                match polled {
                    Ok(None) => {}
                    Ok(Some(s)) => break Ok(s),
                    Err(e) => break Err(e),
                }
                thread::sleep(Duration::from_millis(25));
            };
            if !finished {
                // Give the stderr reader a moment to finish.
                thread::sleep(Duration::from_millis(50));
                let tail: Vec<_> = tail.lock().unwrap().iter().cloned().collect();
                let error = match status {
                    Ok(s) if s.success() => None,
                    Ok(s) => Some(if tail.is_empty() {
                        format!("{} exited with {s}", adapter.kind().label())
                    } else {
                        tail.join("\n")
                    }),
                    Err(e) => Some(e.to_string()),
                };
                // What the CLI itself said went wrong explains it best.
                let error = adapter.last_error().or(error);
                let _ = tx.send(BackendEvent::Finished {
                    error: error.or(Some("The agent stopped before finishing".into())),
                });
            }
            waiter.lock().unwrap().take();
        })?;
    Ok(RunningTurn { child, events: rx })
}

/// Prepare a backend CLI command: when the editor itself runs under Claude
/// Code, don't let the child think it is a nested session.
fn isolate(command: &mut Command) {
    for var in [
        "CLAUDECODE",
        "CLAUDE_CODE_SESSION_ID",
        "CLAUDE_CODE_ENTRYPOINT",
    ] {
        command.env_remove(var);
    }
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

/// Find a CLI: `$override_var`, else `name` on `PATH` (with `PATHEXT` on
/// Windows). An npm shim (`codex.cmd`) runs its script with Node directly:
/// through `cmd /C`, the system prompt on the command line is cut at
/// cmd's 8191 characters and its quotes, `<`, `>` and `&` are read as
/// shell syntax. Other batch files still run through `cmd /C`. The command
/// opens no console window and has no stdin until the caller sets one.
pub(crate) fn locate(name: &str, override_var: &str) -> std::io::Result<Command> {
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
    if !is_batch {
        return Ok(new_command(path));
    }
    if let Some(target) = shim_target(&path) {
        let is_script = target.extension().is_some_and(|e| {
            ["js", "mjs", "cjs"]
                .iter()
                .any(|js| e.eq_ignore_ascii_case(js))
        });
        if !is_script {
            return Ok(new_command(target));
        }
        let bundled = path.with_file_name("node.exe");
        if let Some(node) = is_file(&bundled)
            .then_some(bundled)
            .or_else(|| search_path("node"))
        {
            let mut c = new_command(node);
            c.arg(target);
            return Ok(c);
        }
    }
    let mut c = new_command("cmd");
    c.arg("/C").arg(path);
    Ok(c)
}

/// What an npm shim runs: the first quoted `%dp0%\…` (or `%~dp0\…`) path in
/// it that exists, resolved against the shim's folder.
fn shim_target(shim: &Path) -> Option<PathBuf> {
    let text = String::from_utf8_lossy(&std::fs::read(shim).ok()?).into_owned();
    let dir = shim.parent()?;
    text.split('"').find_map(|quoted| {
        let rest = quoted
            .strip_prefix("%dp0%")
            .or_else(|| quoted.strip_prefix("%~dp0"))?;
        let rest = rest.trim_start_matches(['\\', '/']);
        if rest.is_empty() || rest.eq_ignore_ascii_case("node.exe") || rest.contains('%') {
            return None;
        }
        let target = dir.join(rest);
        is_file(&target).then_some(target)
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

/// How to start the MCP server: `$FLIKK_MCP`, else `flikk-mcp` next to
/// the running executable, else the editor itself as `<exe> mcp` (so
/// `cargo run -p flikk` works without building `flikk-mcp`).
pub(crate) fn mcp_command() -> std::io::Result<(PathBuf, Vec<String>)> {
    if let Some(p) = std::env::var_os("FLIKK_MCP").filter(|v| !v.is_empty()) {
        return Ok((PathBuf::from(p), Vec::new()));
    }
    let exe = std::env::current_exe()?;
    let name = format!("flikk-mcp{}", std::env::consts::EXE_SUFFIX);
    let candidates = [
        exe.parent().map(|d| d.join(&name)),
        // `target/debug/deps` when run from cargo tooling.
        exe.parent().and_then(|d| d.parent()).map(|d| d.join(&name)),
    ];
    Ok(candidates
        .into_iter()
        .flatten()
        .find(|p| is_file(p))
        .map(|p| (p, Vec::new()))
        .unwrap_or((exe, vec![MCP_SUBCOMMAND.into()])))
}

/// Argument that makes the editor binary run as the stdio MCP server.
pub const MCP_SUBCOMMAND: &str = "mcp";

/// Joins text pieces that arrive as separate messages with a blank line.
#[derive(Default)]
pub(crate) struct TextJoin {
    any: bool,
    pending_break: bool,
}

impl TextJoin {
    /// A new assistant message starts; the next text gets a separator.
    pub(crate) fn new_message(&mut self) {
        if self.any {
            self.pending_break = true;
        }
    }

    pub(crate) fn text(&mut self, text: &str) -> Option<BackendEvent> {
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
