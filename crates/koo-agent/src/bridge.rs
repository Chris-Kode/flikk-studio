//! Local IPC between `koo-mcp` and the editor: newline-delimited JSON over
//! `127.0.0.1`, opened with a per-session token (TECHNICAL_DESIGN §2).
//!
//! `koo-mcp` sends [`BridgeRequest`]s; the editor answers each with a
//! [`BridgeResponse`] carrying the same `id`. Answers may come out of order:
//! view tools finish off-thread.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

/// Environment variables `koo-mcp` reads to find the editor.
pub const ENV_ADDR: &str = "KOO_BRIDGE";
pub const ENV_TOKEN: &str = "KOO_TOKEN";

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

/// MCP `CallToolResult`, forwarded verbatim by `koo-mcp`.
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

    pub fn json(value: &Value) -> Self {
        Self::text(serde_json::to_string_pretty(value).unwrap_or_default())
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
    pub request: IncomingKind,
    pub reply: Reply,
}

pub enum IncomingKind {
    List,
    Call { tool: String, args: Value },
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

/// Editor side: accepts `koo-mcp` connections and queues their requests.
pub struct BridgeServer {
    addr: SocketAddr,
    token: String,
    rx: Receiver<Incoming>,
}

impl BridgeServer {
    pub fn start() -> io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let addr = listener.local_addr()?;
        let token = uuid::Uuid::new_v4().simple().to_string();
        let (tx, rx) = mpsc::channel();
        let expected = token.clone();
        thread::Builder::new()
            .name("koo-bridge".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let (tx, token) = (tx.clone(), expected.clone());
                    let _ = thread::Builder::new()
                        .name("koo-bridge-conn".into())
                        .spawn(move || serve_connection(stream, &token, tx));
                }
            })?;
        Ok(Self { addr, token, rx })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    /// Requests received since the last call; never blocks.
    pub fn drain(&self) -> Vec<Incoming> {
        self.rx.try_iter().collect()
    }
}

fn serve_connection(stream: TcpStream, token: &str, queue: Sender<Incoming>) {
    let _ = stream.set_nodelay(true);
    let Ok(write_half) = stream.try_clone() else {
        return;
    };
    let mut lines = BufReader::new(stream).lines();
    match lines.next().map(|l| l.map(|l| serde_json::from_str(&l))) {
        Some(Ok(Ok(BridgeRequest::Hello { token: t }))) if t == token => {}
        _ => return,
    }
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
        if queue.send(Incoming { request, reply }).is_err() {
            break;
        }
    }
}

/// `koo-mcp` side: one connection, authenticated.
pub fn connect(addr: &str, token: &str) -> io::Result<TcpStream> {
    let mut stream = TcpStream::connect(addr)?;
    stream.set_nodelay(true)?;
    let hello = serde_json::to_string(&BridgeRequest::Hello {
        token: token.into(),
    })?;
    writeln!(stream, "{hello}")?;
    Ok(stream)
}
