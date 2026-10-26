//! Codex CLI in non-interactive mode: `codex exec --json`, continued with
//! `codex exec resume <thread>`. Built from the Codex sources (`exec/cli.rs`,
//! `exec_events.rs`); Codex is not installed where this was written.
//!
//! Codex has no single "MCP only" switch, so the turn turns off what it can:
//! the user's `config.toml` is not loaded (`--ignore-user-config`, which
//! also drops their other MCP servers), the shell, exec, patch and web
//! search tools are disabled, the sandbox is read-only and approvals never
//! prompt. Auth still comes from `CODEX_HOME`.

use super::{Adapter, BackendEvent, TextJoin, TurnSpec, locate};
use koo_core::agent::Backend as BackendKind;
use serde_json::Value;
use std::process::Command;

#[derive(Default)]
pub struct Codex {
    text: TextJoin,
    last_error: Option<String>,
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
        let mut c = locate("codex", "KOOEDITOR_CODEX")?;
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
        let overrides = [
            format!(
                "mcp_servers.koo.command={}",
                toml_str(&turn.mcp.command.to_string_lossy())
            ),
            "mcp_servers.koo.args=[]".into(),
            format!("mcp_servers.koo.env={{{}}}", env.join(",")),
            "mcp_servers.koo.startup_timeout_sec=20".into(),
            "mcp_servers.koo.tool_timeout_sec=300".into(),
            format!("developer_instructions={}", toml_str(&turn.system_prompt)),
            "approval_policy=\"never\"".into(),
            "web_search=\"disabled\"".into(),
            "features.shell_tool=false".into(),
            "features.unified_exec=false".into(),
            "features.apply_patch_freeform=false".into(),
            "features.web_search_request=false".into(),
            "include_apply_patch_tool=false".into(),
            "tools.view_image=false".into(),
        ];
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
