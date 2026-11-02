//! Claude Code, run as `claude -p` with streaming JSON in and out
//! (flags checked against `claude --help`, v2.1.x).
//!
//! The only tools are the `koo` MCP server's (plus `WebSearch` and
//! `WebFetch` when the user allows the web): other built-in tools are
//! removed with `--tools`, other MCP servers are ignored with
//! `--strict-mcp-config`, settings files are not loaded, and `dontAsk`
//! denies anything not allowed.

use super::{Adapter, BackendEvent, TextJoin, TurnSpec, locate};
use koo_core::agent::Backend as BackendKind;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::process::Command;

/// Claude Code's own tools for the web.
const WEB_TOOLS: &str = "WebSearch,WebFetch";

#[derive(Default)]
pub struct Claude {
    text: TextJoin,
    streamed: bool,
    /// Web tool calls waiting for their result, by tool use id.
    web_calls: HashMap<String, (&'static str, Value)>,
}

impl Claude {
    fn web_calls_started(&mut self, message: &Value) -> Vec<BackendEvent> {
        let mut started = Vec::new();
        for block in content(message) {
            let field = |k: &str| block.get(k).and_then(Value::as_str);
            let tool = match field("name") {
                Some("WebSearch") => "web_search",
                Some("WebFetch") => "web_fetch",
                _ => continue,
            };
            if let (Some("tool_use"), Some(id)) = (field("type"), field("id"))
                && !self.web_calls.contains_key(id)
            {
                let input = block.get("input").cloned().unwrap_or_default();
                started.push(BackendEvent::ToolStarted {
                    key: id.to_owned(),
                    tool,
                    summary: web_summary(&input),
                    args: input.clone(),
                });
                self.web_calls.insert(id.to_owned(), (tool, input));
            }
        }
        started
    }

    fn web_calls_finished(&mut self, message: &Value) -> Vec<BackendEvent> {
        content(message)
            .filter_map(|block| {
                let id = block.get("tool_use_id")?.as_str()?;
                let (tool, args) = self.web_calls.remove(id)?;
                let summary = web_summary(&args);
                let failed = block.get("is_error").and_then(Value::as_bool) == Some(true);
                Some(BackendEvent::Tool {
                    key: Some(id.to_owned()),
                    tool,
                    args,
                    summary,
                    ok: !failed,
                })
            })
            .collect()
    }
}

/// What a web call searched for or read.
fn web_summary(args: &Value) -> String {
    ["query", "url"]
        .iter()
        .find_map(|k| args.get(k)?.as_str())
        .unwrap_or_default()
        .to_owned()
}

/// Content blocks of an `assistant` or `user` line.
fn content(message: &Value) -> impl Iterator<Item = &Value> {
    message
        .pointer("/message/content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
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
                    "args": turn.mcp.args,
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
            if turn.web { WEB_TOOLS } else { "" },
            "--allowedTools",
            if turn.web {
                "mcp__koo,WebSearch,WebFetch"
            } else {
                "mcp__koo"
            },
            "--permission-mode",
            "dontAsk",
            "--setting-sources",
            "",
            "--disable-slash-commands",
        ]);
        c.arg("--mcp-config").arg(&config_path);
        c.env(
            "MCP_TOOL_TIMEOUT",
            (super::TOOL_TIMEOUT_SECS * 1000).to_string(),
        );
        c.arg("--system-prompt").arg(&turn.system_prompt);
        if let Some(model) = &turn.model.model {
            c.arg("--model").arg(model);
        }
        if let Some(effort) = &turn.model.effort {
            c.arg("--effort").arg(effort);
        }
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
            Some("assistant") => {
                let started = self.web_calls_started(&v);
                // Without partial messages (older CLIs), take whole messages.
                if self.streamed {
                    return started;
                }
                self.text.new_message();
                let mut out: Vec<BackendEvent> = content(&v)
                    .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                    .filter_map(|b| b.get("text").and_then(Value::as_str))
                    .filter_map(|t| self.text.text(t))
                    .collect();
                out.extend(started);
                out
            }
            Some("user") => self.web_calls_finished(&v),
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
