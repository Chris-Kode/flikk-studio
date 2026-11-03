//! `flikk-mcp`: a stdio MCP server (JSON-RPC 2.0, one message per line) that
//! exposes the editor tools. It holds no state of its own: `tools/list` and
//! `tools/call` are forwarded to the editor over the [`crate::bridge`], and
//! answers are written back as they arrive, so calls may overlap.

use crate::bridge::{self, BridgeRequest, BridgeResponse, ToolResult};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::thread;

const INSTRUCTIONS: &str = "Tools of the Flikk project that is open. Times are timeline seconds, \"M:SS.mmm\", timecode \"HH:MM:SS:FF\" or frames \"f:1234\"; ids are the 8-character ids the tools return. Edits are proposals: they collect in a change set the user reviews in the editor. Effects, transitions and generators are GLSL or HTML programs: list_fx, then reuse one or create_fx, preview_fx to look at it, iterate, then apply.";

/// Newest first; the first is offered when the client asks for something else.
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-11-25", "2025-06-18", "2025-03-26"];

type Out = Arc<Mutex<io::Stdout>>;
type Waiting = Arc<Mutex<HashMap<u64, (Value, Kind)>>>;

#[derive(Clone, Copy)]
enum Kind {
    List,
    Call,
}

fn send(out: &Out, message: &Value) {
    if let Ok(mut out) = out.lock() {
        let _ = writeln!(out, "{message}").and_then(|_| out.flush());
    }
}

fn reply(out: &Out, id: Value, result: Value) {
    send(
        out,
        &json!({ "jsonrpc": "2.0", "id": id, "result": result }),
    );
}

fn reply_error(out: &Out, id: Value, code: i64, message: &str) {
    send(
        out,
        &json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
    );
}

/// Run the server until stdin closes. Reads the editor address and token
/// from [`bridge::ENV_ADDR`] and [`bridge::ENV_TOKEN`] (set when the
/// editor's own agent launches it), else from the `bridge.json` a running
/// editor publishes (an external client such as Claude Code or Codex).
pub fn run() -> anyhow::Result<()> {
    // Without the variables, `bridge.json` is read at connect time: the
    // editor may start (or restart) after this server.
    let fixed = std::env::var(bridge::ENV_ADDR)
        .ok()
        .filter(|a| !a.is_empty())
        .map(|addr| (addr, std::env::var(bridge::ENV_TOKEN).unwrap_or_default()));
    let out: Out = Arc::new(Mutex::new(io::stdout()));
    let waiting: Waiting = Arc::default();

    // Connect lazily so `initialize` works even if the editor is slow.
    let mut editor: Option<TcpStream> = None;
    let mut next_id: u64 = 1;

    for line in io::stdin().lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            reply_error(&out, Value::Null, -32700, "parse error");
            continue;
        };
        let method = message.get("method").and_then(Value::as_str).unwrap_or("");
        let Some(id) = message.get("id").cloned() else {
            continue; // notification
        };
        match method {
            "initialize" => {
                let asked = message
                    .pointer("/params/protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let version = PROTOCOL_VERSIONS
                    .iter()
                    .find(|v| **v == asked)
                    .unwrap_or(&PROTOCOL_VERSIONS[1]);
                reply(
                    &out,
                    id,
                    json!({
                        "protocolVersion": version,
                        "capabilities": { "tools": { "listChanged": false } },
                        "serverInfo": { "name": "flikk", "title": "Flikk", "version": env!("CARGO_PKG_VERSION") },
                        "instructions": INSTRUCTIONS
                    }),
                );
            }
            "ping" => reply(&out, id, json!({})),
            "tools/list" | "tools/call" => {
                if editor.is_none() {
                    let found = fixed
                        .clone()
                        .or_else(|| bridge::Discovery::read().map(|d| (d.addr, d.token)));
                    let Some((addr, token)) = found else {
                        reply_error(
                            &out,
                            id,
                            -32603,
                            &format!(
                                "Flikk is not running (no {})",
                                bridge::Discovery::path().display()
                            ),
                        );
                        continue;
                    };
                    match connect(&addr, &token, out.clone(), waiting.clone()) {
                        Ok(stream) => editor = Some(stream),
                        Err(e) => {
                            reply_error(&out, id, -32603, &format!("editor not reachable: {e}"));
                            continue;
                        }
                    }
                }
                let bridge_id = next_id;
                next_id += 1;
                let request = if method == "tools/list" {
                    BridgeRequest::List { id: bridge_id }
                } else {
                    let params = message.get("params").cloned().unwrap_or(Value::Null);
                    BridgeRequest::Call {
                        id: bridge_id,
                        tool: params
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned(),
                        args: params.get("arguments").cloned().unwrap_or(json!({})),
                    }
                };
                let kind = if method == "tools/list" {
                    Kind::List
                } else {
                    Kind::Call
                };
                waiting
                    .lock()
                    .unwrap()
                    .insert(bridge_id, (id.clone(), kind));
                let line = serde_json::to_string(&request)?;
                let stream = editor.as_mut().unwrap();
                if writeln!(stream, "{line}")
                    .and_then(|_| stream.flush())
                    .is_err()
                {
                    waiting.lock().unwrap().remove(&bridge_id);
                    editor = None;
                    reply_error(&out, id, -32603, "lost the connection to the editor");
                }
            }
            _ => reply_error(&out, id, -32601, "method not found"),
        }
    }
    Ok(())
}

/// Connect and start the thread that turns editor answers into JSON-RPC
/// responses.
fn connect(addr: &str, token: &str, out: Out, waiting: Waiting) -> io::Result<TcpStream> {
    let stream = bridge::connect(addr, token)?;
    let read_half = stream.try_clone()?;
    thread::spawn(move || {
        for line in BufReader::new(read_half).lines() {
            let Ok(line) = line else { break };
            let Ok(response) = serde_json::from_str::<BridgeResponse>(&line) else {
                continue;
            };
            let (bridge_id, result) = match response {
                BridgeResponse::Tools { id, tools } => (id, json!({ "tools": tools })),
                BridgeResponse::Result { id, result } => (id, json!(result)),
            };
            if let Some((rpc_id, _)) = waiting.lock().unwrap().remove(&bridge_id) {
                reply(&out, rpc_id, result);
            }
        }
        // The editor went away: fail whatever is still waiting.
        for (_, (rpc_id, kind)) in waiting.lock().unwrap().drain() {
            match kind {
                Kind::List => reply_error(&out, rpc_id, -32603, "the editor closed"),
                Kind::Call => reply(
                    &out,
                    rpc_id,
                    json!(ToolResult::error("The editor closed the connection.")),
                ),
            }
        }
    });
    Ok(stream)
}
