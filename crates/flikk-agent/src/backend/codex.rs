//! Codex CLI in non-interactive mode: `codex exec --json`, continued with
//! `codex exec resume <thread>`. Built from the Codex sources (`exec/cli.rs`,
//! `exec_events.rs`); Codex is not installed where this was written.
//!
//! Codex has no single "MCP only" switch, so the turn turns off what it can:
//! the user's `config.toml` is not loaded (`--ignore-user-config`, which
//! also drops their other MCP servers), the shell, exec and patch tools are
//! disabled (web search too, unless the user allows it, and image
//! generation always), the sandbox is read-only and approvals never
//! prompt. Flikk's own tools are approved up front, as Flikk asks the user
//! where it matters.
//! Auth still comes from `CODEX_HOME`.

use super::{Adapter, BackendEvent, TextJoin, TurnSpec, locate};
use flikk_core::agent::Backend as BackendKind;
use serde_json::Value;
use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Command;

#[derive(Default)]
pub struct Codex {
    text: TextJoin,
    last_error: Option<String>,
    /// Tool call items begun and not completed, by item id.
    open_calls: HashSet<String>,
}

/// Item types that are a tool call, whose output the thread records.
const CALL_ITEMS: [&str; 4] = [
    "mcp_tool_call",
    "web_search",
    "command_execution",
    "file_change",
];

/// Codex's own folder (`CODEX_HOME`, else `~/.codex`), where its login is.
pub fn codex_home() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(|user| PathBuf::from(user).join(".codex"))
        })
        .unwrap_or_else(|| ".codex".into())
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
            // On by default in Codex; Flikk's own `generate_image` makes
            // pictures instead, with either backend.
            "features.image_generation=false".into(),
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
        let mut out = self.parse_value(&v);
        out.extend(self.track_calls(&v));
        out
    }
}

impl Codex {
    /// Note the tool call items a line begins or completes; how many are
    /// open if that changed.
    fn track_calls(&mut self, v: &Value) -> Option<BackendEvent> {
        let s = |p: &str| v.pointer(p).and_then(Value::as_str);
        if !CALL_ITEMS.contains(&s("/item/type")?) {
            return None;
        }
        let id = s("/item/id")?;
        let changed = match s("/type")? {
            "item.started" => self.open_calls.insert(id.to_owned()),
            "item.completed" => self.open_calls.remove(id),
            _ => false,
        };
        changed.then(|| BackendEvent::OpenCalls(self.open_calls.len()))
    }

    fn parse_value(&mut self, v: &Value) -> Vec<BackendEvent> {
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
