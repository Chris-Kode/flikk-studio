//! Claude Code, run as `claude -p` with streaming JSON in and out
//! (flags checked against `claude --help`, v2.1.x).
//!
//! The only tools are the `koo` MCP server's: built-in tools are removed with
//! `--tools ""`, other MCP servers are ignored with `--strict-mcp-config`,
//! settings files are not loaded, and `dontAsk` denies anything not allowed.

use super::{Adapter, BackendEvent, TextJoin, TurnSpec, locate};
use koo_core::agent::Backend as BackendKind;
use serde_json::{Value, json};
use std::process::Command;

#[derive(Default)]
pub struct Claude {
    text: TextJoin,
    streamed: bool,
}

impl Adapter for Claude {
    fn kind(&self) -> BackendKind {
        BackendKind::Claude
    }

    fn command(&self, turn: &TurnSpec) -> std::io::Result<Command> {
        let mut env = serde_json::Map::new();
        for (k, v) in &turn.mcp.env {
            env.insert(k.clone(), json!(v));
        }
        let config = json!({
            "mcpServers": {
                "koo": {
                    "type": "stdio",
                    "command": turn.mcp.command,
                    "args": [],
                    "env": env,
                }
            }
        });
        let config_path = turn.run_dir.join("mcp.json");
        std::fs::write(&config_path, serde_json::to_vec_pretty(&config)?)?;

        let mut c = locate("claude", "KOOEDITOR_CLAUDE")?;
        c.args([
            "-p",
            "--output-format",
            "stream-json",
            "--input-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--strict-mcp-config",
            "--tools",
            "",
            "--allowedTools",
            "mcp__koo",
            "--permission-mode",
            "dontAsk",
            "--setting-sources",
            "",
            "--disable-slash-commands",
        ]);
        c.arg("--mcp-config").arg(&config_path);
        c.arg("--system-prompt").arg(&turn.system_prompt);
        match &turn.resume {
            Some(session) => c.arg("--resume").arg(session),
            // Name new conversations ourselves rather than trust whatever
            // the CLI would derive from its environment.
            None => c
                .arg("--session-id")
                .arg(uuid::Uuid::new_v4().hyphenated().to_string()),
        };
        Ok(c)
    }

    fn stdin(&self, turn: &TurnSpec) -> String {
        let message = json!({
            "type": "user",
            "message": { "role": "user", "content": turn.message },
            "parent_tool_use_id": null,
        });
        format!("{message}\n")
    }

    fn hold_stdin(&self) -> bool {
        true
    }

    fn parse(&mut self, line: &str) -> Vec<BackendEvent> {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return vec![];
        };
        let s = |p: &str| v.pointer(p).and_then(Value::as_str);
        // Sub-agent output is not part of the reply.
        if v.get("parent_tool_use_id").is_some_and(|p| !p.is_null()) {
            return vec![];
        }
        match s("/type") {
            Some("system") if s("/subtype") == Some("init") => s("/session_id")
                .map(|id| vec![BackendEvent::Session(id.to_owned())])
                .unwrap_or_default(),
            Some("stream_event") => match s("/event/type") {
                Some("message_start") => {
                    self.text.new_message();
                    vec![]
                }
                Some("content_block_delta") if s("/event/delta/type") == Some("text_delta") => {
                    self.streamed = true;
                    s("/event/delta/text")
                        .and_then(|t| self.text.text(t))
                        .into_iter()
                        .collect()
                }
                _ => vec![],
            },
            // Without partial messages (older CLIs), take whole messages.
            Some("assistant") if !self.streamed => {
                self.text.new_message();
                v.pointer("/message/content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                    .filter_map(|b| b.get("text").and_then(Value::as_str))
                    .filter_map(|t| self.text.text(t))
                    .collect()
            }
            Some("result") => {
                let failed = v.get("is_error").and_then(Value::as_bool) == Some(true)
                    || s("/subtype").is_some_and(|st| st != "success");
                let error = failed.then(|| {
                    let errors: Vec<_> = v
                        .get("errors")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .collect();
                    if !errors.is_empty() {
                        errors.join("\n")
                    } else {
                        s("/result")
                            .or(s("/subtype"))
                            .unwrap_or("Claude reported an error")
                            .to_owned()
                    }
                });
                let mut out = Vec::new();
                if let Some(id) = s("/session_id") {
                    out.push(BackendEvent::Session(id.to_owned()));
                }
                out.push(BackendEvent::Finished { error });
                out
            }
            _ => vec![],
        }
    }
}
