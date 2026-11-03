//! The browsers of one render. Screenshots take most of an HTML frame and
//! one page paints one at a time, so each browser captures every few frames.
//! All of them seek through every frame, so scripts run as in one browser.

use crate::worker::{self, Request};
use crate::{HtmlEngine, Key, document, key};
use flikk_core::services::{Frame, FxCall, HtmlCrew, HtmlService, ServiceResult};
use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// More only fight over the CPU: every browser runs every frame's scripts.
const BROWSERS: usize = 3;

type Answer = Receiver<ServiceResult<Frame>>;

/// Runs from [`HtmlCrew::ahead`] to the end of the render.
pub struct Crew {
    engine: Arc<HtmlEngine>,
    requests: Vec<Sender<Request>>,
    threads: Vec<JoinHandle<()>>,
    coming: Mutex<Coming>,
}

#[derive(Default)]
struct Coming {
    /// Frames on their way, in the order they will be asked for.
    frames: VecDeque<(Key, Answer)>,
    /// How many have been handed out.
    dealt: usize,
}

impl Crew {
    pub fn new(engine: Arc<HtmlEngine>) -> Option<Self> {
        let mut requests = Vec::new();
        let mut threads = Vec::new();
        for _ in 0..BROWSERS {
            let (tx, rx) = mpsc::channel();
            let exe = engine.browser().to_path_buf();
            let thread = std::thread::Builder::new()
                .name("flikk-html-crew".into())
                .spawn(move || worker::run(exe, rx))
                .ok()?;
            requests.push(tx);
            threads.push(thread);
        }
        Some(Self {
            engine,
            requests,
            threads,
            coming: Mutex::default(),
        })
    }

    /// The answer for `key`; frames in front of it will not be asked for.
    fn answer(&self, key: &Key) -> Option<Answer> {
        let mut coming = self.coming.lock().unwrap_or_else(|e| e.into_inner());
        let at = coming.frames.iter().position(|(k, _)| k == key)?;
        coming.frames.drain(..at);
        coming.frames.pop_front().map(|(_, answer)| answer)
    }
}

impl HtmlService for Crew {
    fn check(&self, call: &FxCall<'_>) -> ServiceResult<Vec<String>> {
        self.engine.check(call)
    }

    fn frame(&self, call: &FxCall<'_>, width: u32, height: u32) -> ServiceResult<Frame> {
        let job = document::prepare(call);
        match self.answer(&key(&job, call, width, height)) {
            Some(answer) => match answer.recv() {
                Ok(Ok(frame)) => Ok(frame),
                // One more try, in the engine's own browser.
                _ => self.engine.frame(call, width, height),
            },
            None => self.engine.frame(call, width, height),
        }
    }
}

impl HtmlCrew for Crew {
    fn ahead(&self, call: &FxCall<'_>, width: u32, height: u32) {
        let job = Arc::new(document::prepare(call));
        let key = key(&job, call, width, height);
        let mut capture = None;
        {
            let mut coming = self.coming.lock().unwrap_or_else(|e| e.into_inner());
            if !self.engine.cached(&key) && !coming.frames.iter().any(|(k, _)| *k == key) {
                let (reply, answer) = mpsc::channel();
                coming.frames.push_back((key, answer));
                capture = Some((coming.dealt % BROWSERS, reply));
                coming.dealt += 1;
            }
        }
        for (i, requests) in self.requests.iter().enumerate() {
            let job = job.clone();
            let _ = requests.send(match &capture {
                Some((browser, reply)) if *browser == i => Request::Frame {
                    job,
                    width,
                    height,
                    reply: reply.clone(),
                },
                _ => Request::Seek { job, width },
            });
        }
    }
}

impl Drop for Crew {
    fn drop(&mut self) {
        // Closing the channels ends the workers, which kill the browsers.
        self.requests.clear();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}
