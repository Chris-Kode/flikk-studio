//! HTML generators (titles, lower thirds, motion graphics) rendered in
//! headless Chromium with an engine-owned clock, so frames are
//! deterministic, exportable and keep their alpha.
//!
//! The conventions authors follow are in the module docs of
//! [`koo_core::fx`]. Details of this engine:
//! - `window.koo.params` holds numbers, booleans and text as is, colors as
//!   `rgba()` strings, points as `[x, y]` in 0..1 from the top-left and
//!   images as `file://` URLs (empty when unset);
//! - GSAP and its plugins are preloaded with 2D transforms by default;
//!   every copy of GSAP the page holds has its root timeline rendered at
//!   the clip time instead of from its ticker, and keeps finished
//!   animations, and `kooTimeline` runs in a `gsap.context`;
//! - `Math.random` is seeded per load and per `kooTimeline` run, so it
//!   repeats across renders;
//! - `requestAnimationFrame` runs at 60 Hz of clip time while seeking
//!   forward, and once more at the frame's exact time;
//! - seeking backward re-seeks CSS and Web Animations directly, reruns
//!   `kooTimeline` and replays GSAP from 0, and reloads the page when its
//!   scripts used timers or animation frames;
//! - fonts and `<img>` decoding are awaited before every capture.
//!
//! One worker thread owns the browser, launched on first use with its
//! profile under the cache directory and killed with the engine.

mod cdp;
mod document;
mod worker;

use koo_core::services::{Frame, FxCall, HtmlService, ServiceError, ServiceResult};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::mpsc::{self, Sender};
use std::thread::JoinHandle;
use worker::Request;

/// Rendered frames kept for repeated requests (scrubbing, export re-runs).
const CACHE_BYTES: usize = 256 << 20;

pub struct HtmlEngine {
    browser: PathBuf,
    requests: Mutex<Option<Sender<Request>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    cache: Mutex<FrameCache>,
}

impl HtmlEngine {
    /// Find a Chromium-family browser (`KOOEDITOR_CHROME`, `PATH`, the usual
    /// install locations). It starts on the first frame.
    pub fn new() -> anyhow::Result<Self> {
        let browser = cdp::find_browser().ok_or_else(|| {
            anyhow::anyhow!(
                "no Chrome, Chromium, Edge or Brave found; set KOOEDITOR_CHROME to its executable"
            )
        })?;
        Ok(Self {
            browser,
            requests: Mutex::new(None),
            thread: Mutex::new(None),
            cache: Mutex::new(FrameCache::default()),
        })
    }

    /// The browser executable in use.
    pub fn browser(&self) -> &std::path::Path {
        &self.browser
    }

    fn send(&self, request: Request) -> ServiceResult<()> {
        let mut requests = self.requests.lock().unwrap_or_else(|e| e.into_inner());
        if requests.is_none() {
            let (tx, rx) = mpsc::channel();
            let exe = self.browser.clone();
            let handle = std::thread::Builder::new()
                .name("koo-html".into())
                .spawn(move || worker::run(exe, rx))
                .map_err(ServiceError::new)?;
            *requests = Some(tx);
            *self.thread.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle);
        }
        requests
            .as_ref()
            .expect("started")
            .send(request)
            .map_err(|_| ServiceError::new("the HTML engine stopped"))
    }
}

impl HtmlService for HtmlEngine {
    fn check(&self, call: &FxCall<'_>) -> ServiceResult<Vec<String>> {
        let job = document::prepare(call);
        let (reply, answer) = mpsc::channel();
        self.send(Request::Check { job, reply })?;
        answer
            .recv()
            .map_err(|_| ServiceError::new("the HTML engine stopped"))?
    }

    fn frame(&self, call: &FxCall<'_>, width: u32, height: u32) -> ServiceResult<Frame> {
        let job = document::prepare(call);
        let key = (
            job.doc_key,
            job.state_key,
            (call.time.max(0.0) * 1000.0).round() as u64,
            width,
            height,
        );
        if let Some(frame) = self
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
        {
            return Ok(frame);
        }
        let (reply, answer) = mpsc::channel();
        self.send(Request::Frame {
            job,
            width,
            height,
            reply,
        })?;
        let frame = answer
            .recv()
            .map_err(|_| ServiceError::new("the HTML engine stopped"))??;
        self.cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .put(key, frame.clone());
        Ok(frame)
    }
}

impl Drop for HtmlEngine {
    fn drop(&mut self) {
        // Closing the channel ends the worker, which kills the browser.
        self.requests.get_mut().map(Option::take).ok();
        if let Ok(Some(handle)) = self.thread.get_mut().map(Option::take) {
            let _ = handle.join();
        }
    }
}

/// (document, params, milliseconds, width, height)
type Key = (u64, u64, u64, u32, u32);

#[derive(Default)]
struct FrameCache {
    frames: HashMap<Key, (Frame, u64)>,
    bytes: usize,
    tick: u64,
}

impl FrameCache {
    fn get(&mut self, key: &Key) -> Option<Frame> {
        self.tick += 1;
        let entry = self.frames.get_mut(key)?;
        entry.1 = self.tick;
        Some(entry.0.clone())
    }

    fn put(&mut self, key: Key, frame: Frame) {
        let size = frame.rgba.len();
        if size > CACHE_BYTES / 4 {
            return;
        }
        self.tick += 1;
        if let Some((old, _)) = self.frames.insert(key, (frame, self.tick)) {
            self.bytes -= old.rgba.len();
        }
        self.bytes += size;
        while self.bytes > CACHE_BYTES {
            let Some(oldest) = self
                .frames
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(k, _)| *k)
            else {
                break;
            };
            if let Some((f, _)) = self.frames.remove(&oldest) {
                self.bytes -= f.rgba.len();
            }
        }
    }
}
