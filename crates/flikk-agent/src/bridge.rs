//! Local IPC between `flikk-mcp` and the editor: newline-delimited JSON over
//! `127.0.0.1`, opened with a per-session token (TECHNICAL_DESIGN §2).
//!
//! `flikk-mcp` sends [`BridgeRequest`]s; the editor answers each with a
//! [`BridgeResponse`] carrying the same `id`. Answers may come out of order:
//! view tools finish off-thread.
//!
//! Two tokens open a connection. The turn token goes to the `flikk-mcp` the
//! editor's own backend launches. The external token is published in
//! `paths::home()/bridge.json` ([`Discovery`]) so any MCP client on this
//! machine (Claude Code, Codex…) can run `flikk mcp` and drive the
//! running editor; those calls are served as [`Origin::External`].

use flikk_core::paths;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

/// Environment variables `flikk-mcp` reads to find the editor.
pub const ENV_ADDR: &str = "FLIKK_BRIDGE";
pub const ENV_TOKEN: &str = "FLIKK_TOKEN";

/// Who opened a connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Origin {
    /// The `flikk-mcp` of the editor's own agent turn.
    Turn,
    /// An external MCP client; one id per connection.
    External(u64),
}

/// What `bridge.json` holds: where a running editor listens and the token
/// external clients open with.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Discovery {
    pub addr: String,
    pub token: String,
    pub pid: u32,
}

impl Discovery {
    pub fn path() -> PathBuf {
        paths::home().join("bridge.json")
    }

    /// The published editor, if any.
    pub fn read() -> Option<Self> {
        let text = std::fs::read_to_string(Self::path()).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Remove the file if this process published it (on exit; best-effort).
    pub fn remove_if_ours() {
        if Self::read().is_some_and(|d| d.pid == std::process::id()) {
            let _ = std::fs::remove_file(Self::path());
        }
    }

    /// Write the file readable by the user only, replacing it atomically.
    fn write(&self) -> io::Result<PathBuf> {
        let path = Self::path();
        std::fs::create_dir_all(paths::home())?;
        let tmp = path.with_extension(format!("json.{}", self.pid));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(serde_json::to_string_pretty(self)?.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&tmp, &path)?;
        Ok(path)
    }
}

/// MCP `CallToolResult` content.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Content {
    Text {
        text: String,
    },
    Image {
        /// Base64.
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
}

/// MCP `CallToolResult`, forwarded verbatim by `flikk-mcp`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    pub content: Vec<Content>,
    #[serde(rename = "isError", default)]
    pub is_error: bool,
}

impl ToolResult {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![Content::Text { text: text.into() }],
            is_error: false,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            content: vec![Content::Text { text: text.into() }],
            is_error: true,
        }
    }

    /// Compact: indentation costs the model tokens and tells it nothing.
    pub fn json(value: &Value) -> Self {
        Self::text(serde_json::to_string(value).unwrap_or_default())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BridgeRequest {
    Hello { token: String },
    List { id: u64 },
    Call { id: u64, tool: String, args: Value },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BridgeResponse {
    /// MCP tool definitions (`name`, `description`, `inputSchema`).
    Tools {
        id: u64,
        tools: Vec<Value>,
    },
    Result {
        id: u64,
        result: ToolResult,
    },
}

/// One request waiting on the UI thread.
pub struct Incoming {
    pub origin: Origin,
    pub request: IncomingKind,
    pub reply: Reply,
}

pub enum IncomingKind {
    List,
    Call {
        tool: String,
        args: Value,
    },
    /// An external connection closed (its reply goes nowhere).
    Closed,
}

/// Answers one request; cheap to move to a worker thread.
pub struct Reply {
    id: u64,
    tx: Sender<BridgeResponse>,
}

impl Reply {
    pub fn tools(self, tools: Vec<Value>) {
        let _ = self.tx.send(BridgeResponse::Tools { id: self.id, tools });
    }

    pub fn result(self, result: ToolResult) {
        let _ = self.tx.send(BridgeResponse::Result {
            id: self.id,
            result,
        });
    }
}

/// Editor side: accepts `flikk-mcp` connections and queues their requests.
pub struct BridgeServer {
    addr: SocketAddr,
    token: String,
    external_token: String,
    rx: Receiver<Incoming>,
    /// `bridge.json`, while this server has it published.
    published: Option<Discovery>,
}

impl BridgeServer {
    pub fn start() -> io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let addr = listener.local_addr()?;
        let token = uuid::Uuid::new_v4().simple().to_string();
        let external_token = uuid::Uuid::new_v4().simple().to_string();
        let (tx, rx) = mpsc::channel();
        let tokens = Arc::new((token.clone(), external_token.clone()));
        let next_conn = Arc::new(AtomicU64::new(1));
        thread::Builder::new()
            .name("flikk-bridge".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let (tx, tokens, next) = (tx.clone(), tokens.clone(), next_conn.clone());
                    let _ = thread::Builder::new()
                        .name("flikk-bridge-conn".into())
                        .spawn(move || serve_connection(stream, &tokens, &next, tx));
                }
            })?;
        Ok(Self {
            addr,
            token,
            external_token,
            rx,
            published: None,
        })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// For the editor's own `flikk-mcp`.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Write `bridge.json` so external clients can find this editor.
    pub fn publish(&mut self) -> io::Result<PathBuf> {
        let discovery = Discovery {
            addr: self.addr.to_string(),
            token: self.external_token.clone(),
            pid: std::process::id(),
        };
        let path = discovery.write()?;
        self.published = Some(discovery);
        Ok(path)
    }

    /// Remove `bridge.json` if it is still ours (best-effort).
    pub fn unpublish(&mut self) {
        if let Some(mine) = self.published.take()
            && Discovery::read().is_some_and(|d| d == mine)
        {
            let _ = std::fs::remove_file(Discovery::path());
        }
    }

    /// Requests received since the last call; never blocks.
    pub fn drain(&self) -> Vec<Incoming> {
        self.rx.try_iter().collect()
    }
}

impl Drop for BridgeServer {
    fn drop(&mut self) {
        self.unpublish();
    }
}

fn serve_connection(
    stream: TcpStream,
    tokens: &(String, String),
    next_conn: &AtomicU64,
    queue: Sender<Incoming>,
) {
    let _ = stream.set_nodelay(true);
    let Ok(write_half) = stream.try_clone() else {
        return;
    };
    let mut lines = BufReader::new(stream).lines();
    let origin = match lines.next().map(|l| l.map(|l| serde_json::from_str(&l))) {
        Some(Ok(Ok(BridgeRequest::Hello { token: t }))) if t == tokens.0 => Origin::Turn,
        Some(Ok(Ok(BridgeRequest::Hello { token: t }))) if t == tokens.1 => {
            Origin::External(next_conn.fetch_add(1, Ordering::Relaxed))
        }
        _ => return,
    };
    let (tx, rx) = mpsc::channel::<BridgeResponse>();
    thread::spawn(move || {
        let mut out = io::BufWriter::new(write_half);
        for response in rx {
            let Ok(line) = serde_json::to_string(&response) else {
                continue;
            };
            if writeln!(out, "{line}").and_then(|_| out.flush()).is_err() {
                break;
            }
        }
    });
    for line in lines {
        let Ok(line) = line else { break };
        let (id, request) = match serde_json::from_str::<BridgeRequest>(&line) {
            Ok(BridgeRequest::List { id }) => (id, IncomingKind::List),
            Ok(BridgeRequest::Call { id, tool, args }) => (id, IncomingKind::Call { tool, args }),
            _ => continue,
        };
        let reply = Reply { id, tx: tx.clone() };
        if queue
            .send(Incoming {
                origin,
                request,
                reply,
            })
            .is_err()
        {
            return;
        }
    }
    if let Origin::External(_) = origin {
        let _ = queue.send(Incoming {
            origin,
            request: IncomingKind::Closed,
            reply: Reply { id: 0, tx },
        });
    }
}

/// `flikk-mcp` side: one connection, authenticated.
pub fn connect(addr: &str, token: &str) -> io::Result<TcpStream> {
    let mut stream = TcpStream::connect(addr)?;
    stream.set_nodelay(true)?;
    let hello = serde_json::to_string(&BridgeRequest::Hello {
        token: token.into(),
    })?;
    writeln!(stream, "{hello}")?;
    Ok(stream)
}
