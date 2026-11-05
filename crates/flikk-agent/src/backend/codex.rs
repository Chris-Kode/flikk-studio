//! Codex CLI in non-interactive mode: `codex exec --json`, continued with
//! `codex exec resume <thread>`. Built from the Codex sources (`exec/cli.rs`,
//! `exec_events.rs`); Codex is not installed where this was written.
//!
//! Codex has no single "MCP only" switch, so the turn turns off what it can:
//! the user's `config.toml` is not loaded (`--ignore-user-config`, which
//! also drops their other MCP servers), the shell, exec and patch tools are
//! disabled (web search and image generation too, unless the user allows
//! them), the sandbox is read-only and approvals never prompt. Flikk's own
//! tools are approved up front, as Flikk asks the user where it matters.
//! Auth still comes from `CODEX_HOME`.

use super::{Adapter, BackendEvent, TextJoin, TurnSpec, locate};
use flikk_core::agent::Backend as BackendKind;
use serde_json::Value;
use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct Codex {
    text: TextJoin,
    last_error: Option<String>,
}

/// How often a running turn looks for new pictures.
const LOOK_EVERY: Duration = Duration::from_millis(500);
/// How a PNG ends: its last chunk, and that chunk's checksum.
const PNG_END: &[u8] = b"IEND\xAE\x42\x60\x82";

/// Finds the pictures a turn makes. `codex exec --json` never mentions them;
/// Codex saves each to `$CODEX_HOME/generated_images/<thread>/<call>.png`.
pub struct ImageWatch {
    thread: Option<String>,
    seen: HashSet<PathBuf>,
    looked: Instant,
}

impl ImageWatch {
    /// For a turn on `thread`, whose pictures so far are not this turn's.
    pub fn new(thread: Option<String>) -> Self {
        let mut watch = Self {
            thread,
            seen: HashSet::new(),
            looked: Instant::now(),
        };
        watch.fresh();
        watch
    }

    pub fn set_thread(&mut self, thread: &str) {
        self.thread = Some(thread.to_owned());
    }

    pub fn due(&self) -> bool {
        self.looked.elapsed() >= LOOK_EVERY
    }

    /// Pictures finished since the last call, oldest name first.
    pub fn fresh(&mut self) -> Vec<PathBuf> {
        self.looked = Instant::now();
        let Some(thread) = &self.thread else {
            return vec![];
        };
        let Ok(entries) = std::fs::read_dir(images_dir().join(thread)) else {
            return vec![];
        };
        let mut fresh: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "png"))
            .filter(|p| !self.seen.contains(p))
            // Codex may still be writing it.
            .filter(|p| std::fs::read(p).is_ok_and(|bytes| bytes.ends_with(PNG_END)))
            .collect();
        fresh.sort();
        self.seen.extend(fresh.iter().cloned());
        fresh
    }
}

fn images_dir() -> PathBuf {
    let home = std::env::var_os("CODEX_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(|user| PathBuf::from(user).join(".codex"))
        })
        .unwrap_or_else(|| ".codex".into());
    home.join("generated_images")
}

/// A `web_search` item, which also covers opening pages: begun, or
/// finished. Codex reports no outcome for them.
fn web_event(item: &Value, started: bool) -> BackendEvent {
    let s = |p: &str| item.pointer(p).and_then(Value::as_str);
    let url = s("/action/url").unwrap_or_default();
    let (tool, summary) = match s("/action/type") {
        Some("open_page") => ("web_fetch", url.to_owned()),
        Some("find_in_page") => (
            "web_fetch",
            format!("{} in {url}", s("/action/pattern").unwrap_or_default()),
        ),
        _ => {
            let queries: Vec<&str> = item
                .pointer("/action/queries")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let query = s("/query").filter(|q| !q.is_empty());
            let query = query.or(s("/action/query")).map(str::to_owned);
            ("web_search", query.unwrap_or_else(|| queries.join(", ")))
        }
    };
    let key = s("/id").map(str::to_owned);
    let args = item.get("action").cloned().unwrap_or_default();
    match (started, key) {
        (true, Some(key)) => BackendEvent::ToolStarted {
            key,
            tool,
            args,
            summary,
        },
        (_, key) => BackendEvent::Tool {
            key,
            tool,
            args,
            summary,
            ok: true,
        },
    }
}

/// TOML basic string.
fn toml_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

impl Adapter for Codex {
    fn kind(&self) -> BackendKind {
        BackendKind::Codex
    }

    fn command(&self, turn: &TurnSpec) -> std::io::Result<Command> {
        let mut c = locate("codex", "FLIKK_CODEX")?;
        c.args([
            "exec",
            "--json",
            "--skip-git-repo-check",
            "--ignore-user-config",
            "--ignore-rules",
            "--sandbox",
            "read-only",
        ]);
        let env: Vec<String> = turn
            .mcp
            .env
            .iter()
            .map(|(k, v)| format!("{k}={}", toml_str(v)))
            .collect();
        let mut overrides = vec![
            format!(
                "mcp_servers.flikk.command={}",
                toml_str(&turn.mcp.command.to_string_lossy())
            ),
            format!(
                "mcp_servers.flikk.args=[{}]",
                turn.mcp
                    .args
                    .iter()
                    .map(|a| toml_str(a))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            format!("mcp_servers.flikk.env={{{}}}", env.join(",")),
            "mcp_servers.flikk.startup_timeout_sec=20".into(),
            format!(
                "mcp_servers.flikk.tool_timeout_sec={}",
                super::TOOL_TIMEOUT_SECS
            ),
            format!("developer_instructions={}", toml_str(&turn.system_prompt)),
            "approval_policy=\"never\"".into(),
            // Codex asks before an MCP tool that is not read-only, which
            // "never" turns into a refusal. Flikk asks the user itself.
            "mcp_servers.flikk.default_tools_approval_mode=\"approve\"".into(),
            if turn.web {
                "web_search=\"live\"".into()
            } else {
                "web_search=\"disabled\"".into()
            },
            "features.shell_tool=false".into(),
            "features.unified_exec=false".into(),
            "features.apply_patch_freeform=false".into(),
            format!("features.web_search_request={}", turn.web),
            // On by default in Codex.
            format!("features.image_generation={}", turn.image_generation),
            "include_apply_patch_tool=false".into(),
            "tools.view_image=false".into(),
        ];
        if let Some(model) = &turn.model.model {
            overrides.push(format!("model={}", toml_str(model)));
        }
        if let Some(effort) = &turn.model.effort {
            overrides.push(format!("model_reasoning_effort={}", toml_str(effort)));
        }
        for o in overrides {
            c.arg("-c").arg(o);
        }
        if let Some(thread) = &turn.resume {
            c.arg("resume").arg(thread);
        }
        // Prompt from stdin.
        c.arg("-");
        Ok(c)
    }

    fn stdin(&self, turn: &TurnSpec) -> String {
        turn.message.clone()
    }

    fn hold_stdin(&self) -> bool {
        false
    }

    fn parse(&mut self, line: &str) -> Vec<BackendEvent> {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return vec![];
        };
        let s = |p: &str| v.pointer(p).and_then(Value::as_str);
        match s("/type") {
            Some("thread.started") => s("/thread_id")
                .map(|id| vec![BackendEvent::Session(id.to_owned())])
                .unwrap_or_default(),
            Some("item.completed") if s("/item/type") == Some("agent_message") => {
                self.text.new_message();
                s("/item/text")
                    .and_then(|t| self.text.text(t))
                    .into_iter()
                    .collect()
            }
            Some("item.started") if s("/item/type") == Some("web_search") => {
                match web_event(&v["item"], true) {
                    started @ BackendEvent::ToolStarted { .. } => vec![started],
                    _ => vec![],
                }
            }
            Some("item.completed") if s("/item/type") == Some("web_search") => {
                vec![web_event(&v["item"], false)]
            }
            Some("item.completed") if s("/item/type") == Some("error") => {
                self.last_error = s("/item/message").map(str::to_owned);
                vec![]
            }
            Some("turn.completed") => vec![BackendEvent::Finished { error: None }],
            Some("turn.failed") => vec![BackendEvent::Finished {
                error: Some(
                    s("/error/message")
                        .map(str::to_owned)
                        .or(self.last_error.take())
                        .unwrap_or_else(|| "Codex failed".into()),
                ),
            }],
            // Stream errors may be retried by Codex; keep the last one in
            // case the turn ends without `turn.completed`.
            Some("error") => {
                self.last_error = s("/message").map(str::to_owned);
                vec![]
            }
            _ => vec![],
        }
    }
}
