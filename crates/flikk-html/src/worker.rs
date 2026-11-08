//! The thread that owns a browser. Requests arrive over a channel and run
//! one at a time; a few warm tabs keep recently used documents loaded, so
//! repeated frames of one clip only seek and capture.

use crate::cdp::{Browser, Cdp, Event};
use crate::document::{Prepared, file_url};
use anyhow::{Context, Result, anyhow, bail};
use base64::Engine as _;
use flikk_core::services::{Frame, ServiceError, ServiceResult};
use flikk_os::sync::lock;
use serde_json::{Value, json};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const CLOCK: &str = include_str!("clock.js");
/// GSAP 3.15.0 and its motion-graphics plugins, as published on npm, under
/// <https://gsap.com/standard-license>. Loaded after the clock, which takes
/// over its timing as `window.gsap` is set. Transforms stay 2D: GPU layers
/// keep a raster scale from earlier frames, so a frame would depend on the
/// frames before it.
const GSAP: &str = concat!(
    include_str!("gsap/gsap.min.js"),
    "\n",
    include_str!("gsap/CustomEase.min.js"),
    "\n",
    include_str!("gsap/CustomBounce.min.js"),
    "\n",
    include_str!("gsap/CustomWiggle.min.js"),
    "\n",
    include_str!("gsap/EasePack.min.js"),
    "\n",
    include_str!("gsap/SplitText.min.js"),
    "\n",
    include_str!("gsap/TextPlugin.min.js"),
    "\n",
    include_str!("gsap/ScrambleTextPlugin.min.js"),
    "\n",
    include_str!("gsap/DrawSVGPlugin.min.js"),
    "\n",
    include_str!("gsap/MorphSVGPlugin.min.js"),
    "\n",
    include_str!("gsap/MotionPathPlugin.min.js"),
    "\n",
    include_str!("gsap/Physics2DPlugin.min.js"),
    "\n",
    include_str!("gsap/PhysicsPropsPlugin.min.js"),
    "\ngsap.registerPlugin(CustomEase, CustomBounce, CustomWiggle, SplitText, TextPlugin, \
     ScrambleTextPlugin, DrawSVGPlugin, MorphSVGPlugin, MotionPathPlugin, Physics2DPlugin, \
     PhysicsPropsPlugin);\ngsap.config({ force3D: false });\n",
);
const TABS: usize = 4;
const CALL: Duration = Duration::from_secs(10);
const LOAD: Duration = Duration::from_secs(15);
/// How long a browser that failed to start is left before it is started
/// again; requests meanwhile fail with its error.
const RETRY_AFTER: Duration = Duration::from_secs(30);

/// The last launch that failed, shared by every worker of the process:
/// executable, error, when.
static FAILED_LAUNCH: Mutex<Option<(PathBuf, String, Instant)>> = Mutex::new(None);

pub enum Request {
    Frame {
        job: Arc<Prepared>,
        width: u32,
        height: u32,
        reply: Sender<ServiceResult<Frame>>,
    },
    /// Move the page on to a frame another browser captures.
    Seek { job: Arc<Prepared>, width: u32 },
    Check {
        job: Prepared,
        reply: Sender<ServiceResult<Vec<String>>>,
    },
}

struct Tab {
    target: String,
    session: String,
    /// The loaded document.
    doc: u64,
    /// Viewport and device scale last set.
    metrics: (u32, u32, u64),
    used: u64,
}

struct Worker {
    exe: PathBuf,
    browser: Option<Browser>,
    tabs: Vec<Tab>,
    tick: u64,
    /// The target in front: headless only paints that one, so captures in
    /// any other wait forever.
    front: Option<String>,
}

/// Run requests until the channel closes, or until `stop` is set: then
/// those still waiting are dropped unanswered.
pub fn run(exe: PathBuf, requests: Receiver<Request>, stop: Arc<AtomicBool>) {
    let mut worker = Worker {
        exe,
        browser: None,
        tabs: Vec::new(),
        tick: 0,
        front: None,
    };
    for request in requests {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        match request {
            Request::Frame {
                job,
                width,
                height,
                reply,
            } => {
                let result = worker.retry(|w| w.render(&job, width, height));
                let _ = reply.send(result.map_err(|e| ServiceError(format!("{e:#}"))));
            }
            Request::Seek { job, width } => {
                let scale = width as f64 / job.canvas.0.max(1) as f64;
                // The browser that captures this frame reports what is wrong.
                let _ = worker.retry(|w| w.reach(&job, scale));
            }
            Request::Check { job, reply } => {
                let result = worker.retry(|w| w.check(&job));
                let _ = reply.send(result.map_err(|e| ServiceError(format!("{e:#}"))));
            }
        }
    }
}

impl Worker {
    /// Run `f`; if the browser died underneath it, start a new one and try
    /// once more.
    fn retry<T>(&mut self, mut f: impl FnMut(&mut Self) -> Result<T>) -> Result<T> {
        match f(self) {
            Ok(v) => Ok(v),
            Err(e) => {
                if self.alive() {
                    return Err(e);
                }
                self.browser = None;
                self.tabs.clear();
                self.front = None;
                f(self)
            }
        }
    }

    fn alive(&mut self) -> bool {
        self.browser.as_mut().is_some_and(|b| {
            b.cdp
                .call(
                    None,
                    "Browser.getVersion",
                    json!({}),
                    Duration::from_secs(2),
                )
                .is_ok()
        })
    }

    fn render(&mut self, job: &Prepared, width: u32, height: u32) -> Result<Frame> {
        if width == 0 || height == 0 {
            bail!("frame size must not be zero");
        }
        let scale = width as f64 / job.canvas.0.max(1) as f64;
        let i = self.reach(job, scale)?;
        let png = self.capture(i);
        if png.is_err() {
            self.discard(i)?;
        }
        let (w, h, rgba) = decode_png(&png?)?;
        let rgba = if (w, h) == (width, height) {
            rgba
        } else {
            resize(&rgba, w, h, width, height)
        };
        Ok(Frame {
            width,
            height,
            rgba: rgba.into(),
        })
    }

    /// The tab of `job`'s document, sought to its time.
    fn reach(&mut self, job: &Prepared, scale: f64) -> Result<usize> {
        for _ in 0..2 {
            let i = self.tab_for(job, scale)?;
            self.tick += 1;
            self.tabs[i].used = self.tick;
            let sought = self.seek(i, job, scale);
            if !matches!(sought, Ok(true)) {
                // Scripts can't go back in time: start over in a new tab. A
                // tab that failed may be wedged (a page in an endless loop).
                self.discard(i)?;
            }
            if sought? {
                return Ok(i);
            }
        }
        bail!("the page could not seek to {} s", job.state["time"])
    }

    fn discard(&mut self, i: usize) -> Result<()> {
        let tab = self.tabs.remove(i);
        close(cdp(&mut self.browser, &self.exe)?, &tab);
        Ok(())
    }

    /// Seek tab `i`; false when the page needs a reload.
    fn seek(&mut self, i: usize, job: &Prepared, scale: f64) -> Result<bool> {
        let conn = cdp(&mut self.browser, &self.exe)?;
        let tab = &mut self.tabs[i];
        if self.front.as_deref() != Some(tab.target.as_str()) {
            self.front = None;
            conn.call(Some(&tab.session), "Page.bringToFront", json!({}), CALL)?;
            self.front = Some(tab.target.clone());
        }
        metrics(conn, tab, job.canvas, scale)?;
        Ok(seek(conn, tab, job)? != "reload")
    }

    /// A PNG of tab `i`, which is in front.
    fn capture(&mut self, i: usize) -> Result<Vec<u8>> {
        let conn = cdp(&mut self.browser, &self.exe)?;
        let tab = &self.tabs[i];
        let shot = conn.call(
            Some(&tab.session),
            "Page.captureScreenshot",
            json!({ "format": "png", "optimizeForSpeed": true, "fromSurface": true }),
            CALL,
        )?;
        let data = shot
            .get("data")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("the screenshot came back empty"))?;
        Ok(base64::engine::general_purpose::STANDARD.decode(data)?)
    }

    /// A tab with `job`'s document loaded: a warm one, else a new one in
    /// place of the least recently used. Tabs never navigate to a second
    /// document: Chromium can leave layers of a reused page unpainted.
    fn tab_for(&mut self, job: &Prepared, scale: f64) -> Result<usize> {
        if let Some(i) = self.tabs.iter().position(|t| t.doc == job.doc_key) {
            return Ok(i);
        }
        let conn = cdp(&mut self.browser, &self.exe)?;
        if self.tabs.len() >= TABS {
            let lru = (0..self.tabs.len())
                .min_by_key(|&i| self.tabs[i].used)
                .expect("tabs");
            close(conn, &self.tabs.remove(lru));
        }
        self.front = None;
        let mut tab = open(conn)?;
        let loaded =
            metrics(conn, &mut tab, job.canvas, scale).and_then(|()| load(conn, &mut tab, job));
        if let Err(e) = loaded {
            close(conn, &tab);
            return Err(e);
        }
        self.tabs.push(tab);
        Ok(self.tabs.len() - 1)
    }

    /// Load in a fresh tab with console and log reporting on, seek to the
    /// call's time and collect what went wrong.
    fn check(&mut self, job: &Prepared) -> Result<Vec<String>> {
        self.front = None;
        let cdp = cdp(&mut self.browser, &self.exe)?;
        let mut tab = open(cdp)?;
        let result = (|| {
            cdp.call(Some(&tab.session), "Runtime.enable", json!({}), CALL)?;
            cdp.call(Some(&tab.session), "Log.enable", json!({}), CALL)?;
            metrics(cdp, &mut tab, job.canvas, 1.0)?;
            let mut problems = Vec::new();
            match load(cdp, &mut tab, job) {
                Ok(()) => {
                    if let Err(e) = seek(cdp, &mut tab, job) {
                        problems.push(format!("{e:#}"));
                    }
                    let errors = cdp.call(
                        Some(&tab.session),
                        "Runtime.evaluate",
                        json!({
                            "expression": "window.__flikk ? window.__flikk.errors : []",
                            "returnByValue": true,
                        }),
                        CALL,
                    )?;
                    if let Some(list) = errors.pointer("/result/value").and_then(Value::as_array) {
                        for e in list.iter().filter_map(Value::as_str) {
                            problems.push(stack_message(e, job));
                        }
                    }
                }
                Err(e) => problems.push(format!("{e:#}")),
            }
            cdp.drain(Duration::from_millis(30))?;
            let events = cdp.take_events(&tab.session);
            let mut found: Vec<String> = events.iter().filter_map(|e| describe(e, job)).collect();
            found.append(&mut problems);
            let mut unique = Vec::new();
            for p in found {
                if !unique.contains(&p) {
                    unique.push(p);
                }
            }
            Ok(unique)
        })();
        close(cdp, &tab);
        result
    }
}

/// The browser's connection, launching it on first use. A launch that
/// failed less than [`RETRY_AFTER`] ago is not tried again: its error is
/// returned instead.
fn cdp<'a>(browser: &'a mut Option<Browser>, exe: &Path) -> Result<&'a mut Cdp> {
    if browser.is_none() {
        let failed = lock(&FAILED_LAUNCH);
        if let Some((_, error, _)) = failed
            .as_ref()
            .filter(|(path, _, at)| path == exe && at.elapsed() < RETRY_AFTER)
        {
            bail!("{error}");
        }
        drop(failed);
        let launched = Browser::launch(exe);
        let mut failed = lock(&FAILED_LAUNCH);
        match launched {
            Ok(b) => {
                if failed.as_ref().is_some_and(|(path, ..)| path == exe) {
                    *failed = None;
                }
                *browser = Some(b);
            }
            Err(e) => {
                let error = format!("{e:#}");
                *failed = Some((exe.to_owned(), error.clone(), Instant::now()));
                bail!("{error}");
            }
        }
    }
    Ok(&mut browser.as_mut().expect("launched").cdp)
}

fn open(cdp: &mut Cdp) -> Result<Tab> {
    let target = cdp.call(
        None,
        "Target.createTarget",
        json!({ "url": "about:blank" }),
        CALL,
    )?;
    let target = target
        .get("targetId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("no target id"))?
        .to_owned();
    let session = cdp.call(
        None,
        "Target.attachToTarget",
        json!({ "targetId": target, "flatten": true }),
        CALL,
    )?;
    let session = session
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("no session id"))?
        .to_owned();
    let s = Some(session.as_str());
    cdp.call(s, "Page.enable", json!({}), CALL)?;
    for source in [CLOCK, GSAP] {
        cdp.call(
            s,
            "Page.addScriptToEvaluateOnNewDocument",
            json!({ "source": source }),
            CALL,
        )?;
    }
    cdp.call(
        s,
        "Emulation.setDefaultBackgroundColorOverride",
        json!({ "color": { "r": 0, "g": 0, "b": 0, "a": 0 } }),
        CALL,
    )?;
    Ok(Tab {
        target,
        session,
        doc: 0,
        metrics: (0, 0, 0),
        used: 0,
    })
}

fn close(cdp: &mut Cdp, tab: &Tab) {
    let _ = cdp.call(
        None,
        "Target.closeTarget",
        json!({ "targetId": tab.target }),
        Duration::from_secs(2),
    );
    cdp.take_events(&tab.session);
}

fn metrics(cdp: &mut Cdp, tab: &mut Tab, canvas: (u32, u32), scale: f64) -> Result<()> {
    let (width, height) = (canvas.0.max(1), canvas.1.max(1));
    let want = (width, height, scale.to_bits());
    if tab.metrics != want {
        cdp.call(
            Some(&tab.session),
            "Emulation.setDeviceMetricsOverride",
            json!({
                "width": width,
                "height": height,
                "deviceScaleFactor": scale,
                "mobile": false,
            }),
            CALL,
        )?;
        tab.metrics = want;
    }
    Ok(())
}

fn load(cdp: &mut Cdp, tab: &mut Tab, job: &Prepared) -> Result<()> {
    let dir = flikk_core::paths::cache().join("html");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{:016x}.html", job.doc_key));
    write_page(&path, job.doc.as_bytes()).context("writing the page")?;
    let s = Some(tab.session.as_str());
    cdp.call(
        s,
        "Page.addScriptToEvaluateOnNewDocument",
        json!({ "source": format!("window.__flikk && window.__flikk.apply({});", job.state) }),
        CALL,
    )?;
    // Forget load events of earlier navigations, keep errors for `check`.
    let session = tab.session.clone();
    let mut pending = cdp.take_events(&session);
    pending.retain(|e| e.method != "Page.loadEventFired");
    let url = file_url(&path.to_string_lossy());
    let nav = cdp.call(s, "Page.navigate", json!({ "url": url }), CALL)?;
    if let Some(err) = nav.get("errorText").and_then(Value::as_str) {
        bail!("loading the page: {err}");
    }
    let loaded = cdp.wait_event(&session, "Page.loadEventFired", LOAD)?;
    cdp.restore(pending);
    if loaded.is_none() {
        bail!(
            "the page did not finish loading within {} s",
            LOAD.as_secs()
        );
    }
    tab.doc = job.doc_key;
    Ok(())
}

/// Make `path` hold `doc`, unless it does already. Written to a file of its
/// own and renamed into place, so other workers loading the same document,
/// and later sessions after a crash, never see a partial page.
fn write_page(path: &Path, doc: &[u8]) -> std::io::Result<()> {
    let holds = |path: &Path| std::fs::read(path).is_ok_and(|bytes| bytes == doc);
    if holds(path) {
        return Ok(());
    }
    static WRITES: AtomicU64 = AtomicU64::new(0);
    let n = WRITES.fetch_add(1, Ordering::Relaxed);
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let temp = path.with_file_name(format!("tmp-{}-{n}-{name}", std::process::id()));
    let written = std::fs::write(&temp, doc).and_then(|()| std::fs::rename(&temp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
        // Another worker may have put the same page in place meanwhile
        // (Windows refuses to replace a file that is open).
        if holds(path) {
            return Ok(());
        }
    }
    written
}

/// `__flikk.frame(state)`: "ok", or "reload" when JS state cannot go back.
fn seek(cdp: &mut Cdp, tab: &mut Tab, job: &Prepared) -> Result<String> {
    let r = cdp.call(
        Some(&tab.session),
        "Runtime.evaluate",
        json!({
            "expression": format!("window.__flikk.frame({})", job.state),
            "awaitPromise": true,
            "returnByValue": true,
        }),
        CALL,
    )?;
    if let Some(details) = r.get("exceptionDetails") {
        bail!("{}", exception_text(details, job));
    }
    Ok(r.pointer("/result/value")
        .and_then(Value::as_str)
        .unwrap_or("ok")
        .to_owned())
}

fn describe(event: &Event, job: &Prepared) -> Option<String> {
    let p = &event.params;
    match event.method.as_str() {
        "Runtime.exceptionThrown" => Some(exception_text(p.get("exceptionDetails")?, job)),
        "Runtime.consoleAPICalled" => {
            let kind = p.get("type").and_then(Value::as_str)?;
            if kind != "error" && kind != "assert" {
                return None;
            }
            let args: Vec<String> = p
                .get("args")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|a| match a.get("value") {
                    Some(Value::String(s)) => s.clone(),
                    Some(v) => v.to_string(),
                    None => a
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                })
                .collect();
            let line = p
                .pointer("/stackTrace/callFrames/0/lineNumber")
                .and_then(Value::as_u64);
            Some(format!(
                "console.error: {}{}",
                args.join(" "),
                at_line(line.map(|l| l as usize + 1), job)
            ))
        }
        "Log.entryAdded" => {
            let entry = p.get("entry")?;
            if entry.get("level").and_then(Value::as_str) != Some("error") {
                return None;
            }
            let text = entry.get("text").and_then(Value::as_str).unwrap_or("");
            match entry.get("url").and_then(Value::as_str) {
                Some(url) if entry.get("source").and_then(Value::as_str) == Some("network") => {
                    Some(format!("{text} ({url})"))
                }
                _ => Some(text.to_owned()),
            }
        }
        _ => None,
    }
}

fn exception_text(details: &Value, job: &Prepared) -> String {
    let text = details
        .pointer("/exception/description")
        .and_then(Value::as_str)
        .and_then(|d| d.lines().next())
        .or_else(|| details.get("text").and_then(Value::as_str))
        .unwrap_or("uncaught exception");
    let line = details
        .get("lineNumber")
        .and_then(Value::as_u64)
        .map(|l| l as usize + 1);
    let ours = details
        .get("url")
        .and_then(Value::as_str)
        .is_some_and(|u| u.ends_with(".html"));
    format!(
        "{text}{}",
        if ours {
            at_line(line, job)
        } else {
            String::new()
        }
    )
}

/// First line of a caught error's stack, with the author's line number.
fn stack_message(stack: &str, job: &Prepared) -> String {
    let mut lines = stack.lines();
    let first = lines.next().unwrap_or("error").trim().to_owned();
    let line = lines.find_map(|l| {
        let at = l.find(".html:")?;
        l[at + 6..].split(':').next()?.parse::<usize>().ok()
    });
    format!("{first}{}", at_line(line, job))
}

/// " (line N)" in the author's numbering, when it falls inside their code.
fn at_line(line: Option<usize>, job: &Prepared) -> String {
    match line {
        Some(l) if l > job.header_lines => format!(" (line {})", l - job.header_lines),
        _ => String::new(),
    }
}

fn decode_png(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>)> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info()?;
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| anyhow!("screenshot too large"))?;
    let mut buf = vec![0; size];
    let info = reader.next_frame(&mut buf)?;
    buf.truncate(info.buffer_size());
    let (w, h) = (info.width, info.height);
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|&[r, g, b]| [r, g, b, 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => buf
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|&[g, a]| [g, g, g, a])
            .collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => bail!("unexpected indexed screenshot"),
    };
    Ok((w, h, rgba))
}

/// Bilinear resize of straight-alpha RGBA, weighting color by alpha so
/// transparent edges don't darken.
fn resize(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let mut out = vec![0u8; (dw * dh * 4) as usize];
    let px = |x: u32, y: u32| {
        let i = ((y.min(sh - 1) * sw + x.min(sw - 1)) * 4) as usize;
        [src[i], src[i + 1], src[i + 2], src[i + 3]]
    };
    for y in 0..dh {
        let fy = ((y as f32 + 0.5) * sh as f32 / dh as f32 - 0.5).max(0.0);
        let (y0, ty) = (fy.floor() as u32, fy.fract());
        for x in 0..dw {
            let fx = ((x as f32 + 0.5) * sw as f32 / dw as f32 - 0.5).max(0.0);
            let (x0, tx) = (fx.floor() as u32, fx.fract());
            let taps = [
                (px(x0, y0), (1.0 - tx) * (1.0 - ty)),
                (px(x0 + 1, y0), tx * (1.0 - ty)),
                (px(x0, y0 + 1), (1.0 - tx) * ty),
                (px(x0 + 1, y0 + 1), tx * ty),
            ];
            let mut acc = [0f32; 4];
            for (p, w) in taps {
                let a = p[3] as f32 * w;
                acc[0] += p[0] as f32 * a;
                acc[1] += p[1] as f32 * a;
                acc[2] += p[2] as f32 * a;
                acc[3] += a;
            }
            let i = ((y * dw + x) * 4) as usize;
            if acc[3] > 0.0 {
                for c in 0..3 {
                    out[i + c] = (acc[c] / acc[3]).round().clamp(0.0, 255.0) as u8;
                }
            }
            out[i + 3] = acc[3].round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}
