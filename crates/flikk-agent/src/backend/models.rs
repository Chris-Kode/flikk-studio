//! The models each backend CLI offers, asked from the CLI itself so the
//! list follows the installed version: Claude Code answers the `initialize`
//! control request of its stream-JSON mode, Codex the `model/list` request
//! of `codex app-server`. Neither starts a turn. The last answer is kept in
//! `paths::home()/agent/models.json` for the next launch.

use super::{isolate, locate};
use crate::session;
use flikk_core::agent::{AgentModel, Backend};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

/// How long a CLI gets to answer.
const TIMEOUT: Duration = Duration::from_secs(20);
/// Codex pages its list; stop after this many pages.
const MAX_PAGES: u64 = 10;

fn file() -> PathBuf {
    session::agent_dir().join("models.json")
}

fn key(backend: Backend) -> String {
    backend.label().to_lowercase()
}

fn load_all() -> BTreeMap<String, Vec<AgentModel>> {
    std::fs::read(file())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_all(all: &BTreeMap<String, Vec<AgentModel>>) {
    if let Ok(bytes) = serde_json::to_vec_pretty(all) {
        let _ = flikk_os::fs::write_atomic(&file(), &bytes);
    }
}

/// The list `backend`'s CLI gave last time, empty if it never answered.
pub fn cached_models(backend: Backend) -> Vec<AgentModel> {
    load_all().remove(&key(backend)).unwrap_or_default()
}

/// Ask `backend`'s CLI for its models and remember the answer. Blocks for
/// up to [`TIMEOUT`]; run it off the UI thread.
pub fn refresh_models(backend: Backend) -> Result<Vec<AgentModel>, String> {
    let models = match backend {
        Backend::Claude => claude(),
        Backend::Codex => codex(),
    }
    .map_err(|e| e.to_string())?;
    if models.is_empty() {
        return Err(format!("{} listed no models", backend.label()));
    }
    let mut all = load_all();
    all.insert(key(backend), models.clone());
    save_all(&all);
    Ok(models)
}

fn claude() -> io::Result<Vec<AgentModel>> {
    let mut c = locate("claude", "FLIKK_CLAUDE")?;
    c.args([
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--strict-mcp-config",
        "--tools",
        "",
        "--setting-sources",
        "",
        "--no-session-persistence",
    ]);
    let ask = json!({
        "type": "control_request",
        "request_id": "flikk-models",
        "request": { "subtype": "initialize" },
    });
    exchange(c, &[ask], |v, _| {
        if v["type"] != "control_response" || v["response"]["request_id"] != "flikk-models" {
            return Ok(None);
        }
        if v["response"]["subtype"] == "error" {
            return Err(io::Error::other(
                v["response"]["error"]
                    .as_str()
                    .unwrap_or("Claude did not list its models")
                    .to_owned(),
            ));
        }
        Ok(Some(claude_models(&v["response"]["response"]["models"])))
    })
}

fn claude_models(list: &Value) -> Vec<AgentModel> {
    let entries = list.as_array().map(Vec::as_slice).unwrap_or_default();
    // "default" stands for one of the other entries; flag that one instead.
    let mut default = entries
        .iter()
        .find(|m| m["value"] == "default")
        .and_then(|m| m["resolvedModel"].as_str());
    entries
        .iter()
        .filter(|m| m["value"] != "default")
        .filter_map(|m| {
            let id = m["value"].as_str()?;
            let is_default = default.is_some() && m["resolvedModel"].as_str() == default;
            if is_default {
                default = None;
            }
            Some(AgentModel {
                id: id.to_owned(),
                label: m["displayName"].as_str().unwrap_or(id).to_owned(),
                efforts: strings(&m["supportedEffortLevels"]),
                default: is_default,
            })
        })
        .collect()
}

fn codex() -> io::Result<Vec<AgentModel>> {
    let mut c = locate("codex", "FLIKK_CODEX")?;
    c.arg("app-server");
    let init = json!({
        "method": "initialize",
        "id": 0,
        "params": {
            "clientInfo": {
                "name": "flikk",
                "title": "Flikk",
                "version": env!("CARGO_PKG_VERSION"),
            }
        },
    });
    let list = |id: u64, cursor: Option<&str>| {
        json!({
            "method": "model/list",
            "id": id,
            "params": { "includeHidden": false, "cursor": cursor },
        })
    };
    let mut models = Vec::new();
    exchange(c, &[init], |v, stdin| {
        // Skip notifications and requests from the server.
        let Some(id) = v["id"].as_u64().filter(|_| v.get("method").is_none()) else {
            return Ok(None);
        };
        if let Some(error) = v.get("error") {
            return Err(io::Error::other(
                error["message"]
                    .as_str()
                    .unwrap_or("Codex did not list its models")
                    .to_owned(),
            ));
        }
        if id == 0 {
            send(stdin, &json!({ "method": "initialized", "params": {} }))?;
            send(stdin, &list(1, None))?;
            return Ok(None);
        }
        let result = &v["result"];
        models.extend(codex_models(&result["data"]));
        match result["nextCursor"].as_str() {
            Some(cursor) if id < MAX_PAGES => {
                send(stdin, &list(id + 1, Some(cursor)))?;
                Ok(None)
            }
            _ => Ok(Some(std::mem::take(&mut models))),
        }
    })
}

fn codex_models(list: &Value) -> Vec<AgentModel> {
    let entries = list.as_array().map(Vec::as_slice).unwrap_or_default();
    entries
        .iter()
        .filter_map(|m| {
            let id = m["model"].as_str().or(m["id"].as_str())?;
            Some(AgentModel {
                id: id.to_owned(),
                label: m["displayName"].as_str().unwrap_or(id).to_owned(),
                efforts: m["supportedReasoningEfforts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|e| e["reasoningEffort"].as_str().map(str::to_owned))
                    .collect(),
                default: m["isDefault"].as_bool() == Some(true),
            })
        })
        .collect()
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s.as_str().map(str::to_owned))
        .collect()
}

fn send(stdin: &mut ChildStdin, message: &Value) -> io::Result<()> {
    writeln!(stdin, "{message}")?;
    stdin.flush()
}

/// Run `command` as a JSON-lines peer: send `first`, then hand each message
/// it prints to `on_message` (which may answer through stdin) until that
/// returns a result. The process is killed either way.
fn exchange<T>(
    mut command: Command,
    first: &[Value],
    mut on_message: impl FnMut(&Value, &mut ChildStdin) -> io::Result<Option<T>>,
) -> io::Result<T> {
    let workdir = session::agent_dir().join("work");
    std::fs::create_dir_all(&workdir)?;
    command
        .current_dir(&workdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    isolate(&mut command);
    let mut child = command.spawn()?;
    let (mut stdin, stdout) = (child.stdin.take().unwrap(), child.stdout.take().unwrap());
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + TIMEOUT;
    let result = (|| {
        for message in first {
            send(&mut stdin, message)?;
        }
        loop {
            let timeout = deadline.saturating_duration_since(Instant::now());
            let line = match rx.recv_timeout(timeout) {
                Ok(line) => line,
                Err(RecvTimeoutError::Timeout) => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "timed out waiting for the model list",
                    ));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::other("the CLI exited before listing its models"));
                }
            };
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if let Some(out) = on_message(&message, &mut stdin)? {
                return Ok(out);
            }
        }
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}
