//! A small synchronous Chrome DevTools Protocol client over the browser's
//! websocket, with flattened target sessions, and the headless browser
//! process it talks to.

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tungstenite::protocol::WebSocketConfig;
use tungstenite::{Message, WebSocket};

/// Events worth keeping until someone asks; everything else is dropped.
const KEPT: &[&str] = &[
    "Page.loadEventFired",
    "Runtime.exceptionThrown",
    "Runtime.consoleAPICalled",
    "Log.entryAdded",
];
const MAX_EVENTS: usize = 2000;

pub struct Event {
    pub session: Option<String>,
    pub method: String,
    pub params: Value,
}

pub struct Cdp {
    ws: WebSocket<TcpStream>,
    next_id: u64,
    events: VecDeque<Event>,
}

impl Cdp {
    fn connect(port: u16, path: &str) -> Result<Self> {
        let stream =
            TcpStream::connect(("127.0.0.1", port)).context("connecting to the browser")?;
        stream.set_nodelay(true)?;
        let config = WebSocketConfig::default()
            .max_message_size(None)
            .max_frame_size(None);
        let url = format!("ws://127.0.0.1:{port}{path}");
        let (ws, _) = tungstenite::client::client_with_config(url, stream, Some(config))
            .map_err(|e| anyhow!("devtools handshake: {e}"))?;
        ws.get_ref()
            .set_read_timeout(Some(Duration::from_millis(50)))?;
        Ok(Self {
            ws,
            next_id: 1,
            events: VecDeque::new(),
        })
    }

    /// Send a command and wait for its result.
    pub fn call(
        &mut self,
        session: Option<&str>,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let mut msg = json!({ "id": id, "method": method, "params": params });
        if let Some(s) = session {
            msg["sessionId"] = json!(s);
        }
        self.ws
            .send(Message::text(msg.to_string()))
            .map_err(|e| anyhow!("devtools send: {e}"))?;
        let deadline = Instant::now() + timeout;
        loop {
            let Some(msg) = self.read(deadline)? else {
                bail!("{method} timed out after {:.0} s", timeout.as_secs_f64());
            };
            if msg.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(err) = msg.get("error") {
                    let text = err.get("message").and_then(Value::as_str).unwrap_or("");
                    bail!("{method}: {text}");
                }
                return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
            }
            self.keep(msg);
        }
    }

    /// Wait for `method` on `session`; `None` on timeout.
    pub fn wait_event(
        &mut self,
        session: &str,
        method: &str,
        timeout: Duration,
    ) -> Result<Option<Event>> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(i) = self
                .events
                .iter()
                .position(|e| e.method == method && e.session.as_deref() == Some(session))
            {
                return Ok(self.events.remove(i));
            }
            match self.read(deadline)? {
                Some(msg) => self.keep(msg),
                None => return Ok(None),
            }
        }
    }

    /// Read whatever arrives within `wait`, keeping events.
    pub fn drain(&mut self, wait: Duration) -> Result<()> {
        let deadline = Instant::now() + wait;
        while let Some(msg) = self.read(deadline)? {
            self.keep(msg);
        }
        Ok(())
    }

    /// Remove and return `session`'s kept events.
    pub fn take_events(&mut self, session: &str) -> Vec<Event> {
        let (mine, rest): (VecDeque<_>, VecDeque<_>) = std::mem::take(&mut self.events)
            .into_iter()
            .partition(|e| e.session.as_deref() == Some(session));
        self.events = rest;
        mine.into()
    }

    /// Put back events taken with [`Self::take_events`], ahead of newer ones.
    pub fn restore(&mut self, events: Vec<Event>) {
        for e in events.into_iter().rev() {
            self.events.push_front(e);
        }
    }

    fn keep(&mut self, msg: Value) {
        let Some(method) = msg.get("method").and_then(Value::as_str) else {
            return;
        };
        if !KEPT.contains(&method) {
            return;
        }
        if self.events.len() >= MAX_EVENTS {
            self.events.pop_front();
        }
        self.events.push_back(Event {
            session: msg
                .get("sessionId")
                .and_then(Value::as_str)
                .map(str::to_owned),
            method: method.to_owned(),
            params: msg.get("params").cloned().unwrap_or(Value::Null),
        });
    }

    /// Next JSON message, or `None` once `deadline` passes.
    fn read(&mut self, deadline: Instant) -> Result<Option<Value>> {
        loop {
            match self.ws.read() {
                Ok(Message::Text(text)) => return Ok(Some(serde_json::from_str(&text)?)),
                Ok(Message::Binary(bytes)) => return Ok(Some(serde_json::from_slice(&bytes)?)),
                Ok(Message::Close(_)) => bail!("the browser closed the connection"),
                Ok(_) => {}
                Err(tungstenite::Error::Io(e))
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    if Instant::now() >= deadline {
                        return Ok(None);
                    }
                }
                Err(e) => bail!("devtools connection: {e}"),
            }
        }
    }
}

/// A headless browser process; killed and its profile removed on drop.
pub struct Browser {
    child: Child,
    profile: PathBuf,
    pub cdp: Cdp,
}

impl Browser {
    pub fn launch(exe: &Path) -> Result<Self> {
        static LAUNCHES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = LAUNCHES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = koo_core::paths::cache().join("html");
        sweep_profiles(&dir);
        let profile = dir.join(format!("profile-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&profile);
        std::fs::create_dir_all(&profile)?;
        let mut cmd = Command::new(exe);
        cmd.arg("--headless")
            .arg("--remote-debugging-port=0")
            .arg(format!("--user-data-dir={}", profile.display()))
            .args([
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-extensions",
                "--disable-background-networking",
                "--disable-background-timer-throttling",
                "--disable-backgrounding-occluded-windows",
                "--disable-renderer-backgrounding",
                "--disable-sync",
                "--disable-component-update",
                "--disable-default-apps",
                "--disable-domain-reliability",
                "--disable-client-side-phishing-detection",
                "--disable-breakpad",
                "--no-pings",
                "--no-service-autorun",
                "--password-store=basic",
                "--use-mock-keychain",
                "--disable-features=Translate,MediaRouter,OptimizationHints",
                "--allow-file-access-from-files",
                "--hide-scrollbars",
                // Software raster without vsync: captures don't wait for a
                // display frame, and pixels match across machines.
                "--disable-gpu",
                "--disable-gpu-compositing",
                "--disable-gpu-vsync",
                "--disable-frame-rate-limit",
                // Animations, image decodes and raster on the main thread
                // before a frame is drawn, so a capture shows the seeked state.
                "--disable-threaded-animation",
                "--disable-threaded-scrolling",
                "--disable-checker-imaging",
                "--mute-audio",
                "--force-color-profile=srgb",
                "about:blank",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if running_as_root() {
            cmd.arg("--no-sandbox");
        }
        let mut child = cmd
            .spawn()
            .with_context(|| format!("starting {}", exe.display()))?;
        let port_file = profile.join("DevToolsActivePort");
        let deadline = Instant::now() + Duration::from_secs(20);
        let (port, path) = loop {
            if let Ok(text) = std::fs::read_to_string(&port_file) {
                let mut lines = text.lines();
                if let (Some(port), Some(path)) = (lines.next(), lines.next())
                    && let Ok(port) = port.trim().parse::<u16>()
                {
                    break (port, path.trim().to_owned());
                }
            }
            if let Some(status) = child.try_wait()? {
                let _ = std::fs::remove_dir_all(&profile);
                bail!("{} exited at startup ({status})", exe.display());
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_dir_all(&profile);
                bail!("{} did not open its debugging port", exe.display());
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let cdp = match Cdp::connect(port, &path) {
            Ok(cdp) => cdp,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_dir_all(&profile);
                return Err(e);
            }
        };
        Ok(Self {
            child,
            profile,
            cdp,
        })
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        // Helper processes may still be writing for a moment.
        for _ in 0..10 {
            if std::fs::remove_dir_all(&self.profile).is_ok() || !self.profile.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// Remove profiles left by editors that exited without cleaning up.
#[cfg(target_os = "linux")]
fn sweep_profiles(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|n| n.strip_prefix("profile-"))
            .and_then(|n| n.split('-').next())
        else {
            continue;
        };
        if !Path::new("/proc").join(pid).exists() {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn sweep_profiles(_dir: &Path) {}

#[cfg(target_os = "linux")]
fn running_as_root() -> bool {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("Uid:"))
                .and_then(|l| l.split_whitespace().nth(1).map(|e| e == "0"))
        })
        .unwrap_or(false)
}

#[cfg(not(target_os = "linux"))]
fn running_as_root() -> bool {
    false
}

/// `KOOEDITOR_CHROME`, then `PATH`, then the usual install locations.
pub fn find_browser() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("KOOEDITOR_CHROME").filter(|v| !v.is_empty()) {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    const NAMES: &[&str] = &[
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "chrome",
        "msedge",
        "microsoft-edge",
        "brave",
        "brave-browser",
    ];
    let exts: &[&str] = if cfg!(windows) { &[".exe"] } else { &[""] };
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            for name in NAMES {
                for ext in exts {
                    let p = dir.join(format!("{name}{ext}"));
                    if p.is_file() {
                        return Some(p);
                    }
                }
            }
        }
    }
    let mut known: Vec<PathBuf> = Vec::new();
    if cfg!(target_os = "macos") {
        for app in [
            "Google Chrome.app/Contents/MacOS/Google Chrome",
            "Chromium.app/Contents/MacOS/Chromium",
            "Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "Brave Browser.app/Contents/MacOS/Brave Browser",
            "Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary",
        ] {
            known.push(Path::new("/Applications").join(app));
            if let Some(home) = std::env::var_os("HOME") {
                known.push(Path::new(&home).join("Applications").join(app));
            }
        }
    }
    if cfg!(windows) {
        let roots = ["ProgramFiles", "ProgramFiles(x86)", "LocalAppData"]
            .iter()
            .filter_map(std::env::var_os);
        for root in roots {
            let root = PathBuf::from(root);
            for rel in [
                r"Google\Chrome\Application\chrome.exe",
                r"Chromium\Application\chrome.exe",
                r"Microsoft\Edge\Application\msedge.exe",
                r"BraveSoftware\Brave-Browser\Application\brave.exe",
            ] {
                known.push(root.join(rel));
            }
        }
    }
    known.into_iter().find(|p| p.is_file())
}
