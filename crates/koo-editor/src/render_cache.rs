//! Background render cache: the ranges that need the compositor (effects,
//! generators, transitions, masks, retimed media…;
//! `compose::needs_compositor`) are composed while the user is idle into
//! frames on disk ([`store`]), at the preview resolution, so playback and
//! scrubbing read them instead of composing heavy frames live.
//!
//! Frames are named by a hash of everything that draws them ([`key`]), so
//! an edit changes only the keys of the frames it changes; frames whose key
//! no longer comes up age out by LRU. Nothing has to be invalidated, and a
//! render cancelled or overtaken by an edit cannot leave a wrong frame
//! under a key that is still asked for.
//!
//! [`Background`] lives in `EditorState` and polls 4 times a second: it
//! plans the shown project once it settles (the timeline's render bar comes
//! from that plan), stops the [`worker`] while playing, scrubbing, editing
//! or delivering, and starts it after [`IDLE`] without any of them.
//! Settings → Storage turns it off, sets the size limit and clears the
//! frames.

mod key;
mod store;
mod worker;

pub use key::{Globals, Plan};
pub use store::{dir, store};

use crate::persistence;
use crate::state::EditorState;
use gpui_kit::*;
use koo_core::model::Project;
use koo_core::{ChangeSetId, Time, TimeRange};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};
use worker::Worker;

const POLL: Duration = Duration::from_millis(250);
/// How long the project must stay as it is before it is planned again.
const SETTLE: Duration = Duration::from_millis(200);
/// How long without playing, scrubbing, editing or delivering before
/// rendering starts.
const IDLE: Duration = Duration::from_millis(1500);
/// How long a render that left frames out (offline media, mattes being
/// made) waits before it tries again.
const RETRY: Duration = Duration::from_secs(5);
/// How often the render bar is brought up to date while frames land.
const STATUS_EVERY: Duration = Duration::from_secs(1);

/// What the cached frames of the shown project depend on, short of their
/// content.
#[derive(Clone, Copy, PartialEq)]
struct Signature {
    version: u64,
    preview: Option<ChangeSetId>,
    draft: usize,
    live: u64,
    globals: Globals,
}

/// Which frames that need the compositor are on disk, in timeline time.
#[derive(Default)]
pub struct Status {
    /// In order: a range and whether it is cached.
    pub runs: Vec<(TimeRange, bool)>,
}

impl Status {
    fn measure(plan: &Plan) -> Self {
        let store = store();
        let mut runs: Vec<(i64, i64, bool)> = Vec::new();
        for span in &plan.spans {
            for frame in span.frames.clone() {
                let cached = store.contains(&span.key(frame));
                match runs.last_mut() {
                    Some((_, end, c)) if *end == frame && *c == cached => *end = frame + 1,
                    _ => runs.push((frame, frame + 1, cached)),
                }
            }
        }
        let t = |f| Time::from_frame(f, plan.rate);
        Self {
            runs: runs
                .into_iter()
                .map(|(a, b, cached)| (TimeRange::new(t(a), t(b)), cached))
                .collect(),
        }
    }

    fn pending(&self) -> bool {
        self.runs.iter().any(|(_, cached)| !cached)
    }
}

pub struct Background {
    seen: Option<Signature>,
    /// When `seen` last changed.
    changed: Instant,
    playhead: Time,
    /// Last play, scrub or edit.
    active: Instant,
    /// The plan of the project as `seen`, and that project.
    plan: Option<(Signature, Arc<Project>, Arc<Plan>)>,
    planning: Option<Signature>,
    status: Option<Arc<Status>>,
    /// The store's version the status was measured at, and when.
    measured: (u64, Instant),
    measuring: bool,
    worker: Option<Worker>,
    retry_at: Option<Instant>,
    _poll: Task<()>,
}

impl Background {
    pub fn new(cx: &mut Context<EditorState>) -> Self {
        let settings = persistence::with_store(|s| s.render_cache_settings()).unwrap_or_default();
        // Reads the folder: off the UI thread.
        cx.background_executor()
            .spawn(async move { store().apply(&settings) })
            .detach();
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                if this.update(cx, poll).is_err() {
                    break;
                }
            }
        });
        let now = Instant::now();
        Self {
            seen: None,
            changed: now,
            playhead: Time::ZERO,
            active: now,
            plan: None,
            planning: None,
            status: None,
            measured: (0, now),
            measuring: false,
            worker: None,
            retry_at: None,
            _poll: poll,
        }
    }

    /// The render bar: which ranges are cached and which still need to be.
    pub fn status(&self) -> Option<Arc<Status>> {
        self.status.clone()
    }
}

/// Settings → Storage changed the background render's settings.
pub fn set_settings(settings: koo_store::RenderCacheSettings) {
    persistence::with_store(|s| s.save_render_cache_settings(&settings));
    store().apply(&settings);
}

pub fn settings() -> koo_store::RenderCacheSettings {
    persistence::with_store(|s| s.render_cache_settings()).unwrap_or_default()
}

/// What cached frames of `state`'s projects share outside them.
pub fn globals(state: &EditorState) -> Globals {
    Globals::new(&state.services, &state.caption_styles())
}

/// `project`'s plan, made once per project snapshot.
pub fn plan_for(project: &Arc<Project>, globals: Globals) -> Arc<Plan> {
    type Plans = Vec<(Weak<Project>, Arc<Plan>)>;
    static PLANS: Mutex<Plans> = Mutex::new(Vec::new());
    let found = PLANS.lock().ok().and_then(|plans| {
        plans
            .iter()
            .find(|(p, plan)| {
                plan.globals == globals && p.upgrade().is_some_and(|p| Arc::ptr_eq(&p, project))
            })
            .map(|(_, plan)| plan.clone())
    });
    if let Some(plan) = found {
        return plan;
    }
    let plan = Arc::new(Plan::new(project, globals));
    if let Ok(mut plans) = PLANS.lock() {
        plans.retain(|(p, _)| p.strong_count() > 0);
        plans.push((Arc::downgrade(project), plan.clone()));
        if plans.len() > 4 {
            plans.remove(0);
        }
    }
    plan
}

/// Frame `frame` of `plan` from disk, if it is there.
pub fn image(plan: &Plan, frame: i64) -> Option<Arc<RenderImage>> {
    let picture = store().read(&plan.key(frame)?)?;
    let buffer = image::RgbaImage::from_raw(picture.width, picture.height, picture.bgra)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}

fn poll(state: &mut EditorState, cx: &mut Context<EditorState>) {
    let now = Instant::now();
    let sig = Signature {
        version: state.bus.version(),
        preview: state.preview,
        draft: state
            .preview
            .and_then(|id| state.bus.change_set(id))
            .map_or(0, |set| set.items.len()),
        live: state.live_version,
        globals: globals(state),
    };
    let busy = state.playing
        || state.source.as_ref().is_some_and(|s| s.is_playing())
        || state.render_queue.is_busy();
    let playhead = state.playhead;
    let bg = &mut state.render;
    if bg.seen != Some(sig) {
        bg.seen = Some(sig);
        bg.changed = now;
        bg.active = now;
        bg.worker = None;
        bg.retry_at = None;
    }
    if bg.playhead != playhead || busy {
        bg.playhead = playhead;
        bg.active = now;
        bg.worker = None;
    }
    if bg.worker.as_ref().is_some_and(Worker::finished) {
        bg.worker = None;
        bg.retry_at = Some(now + RETRY);
    }

    let planned = bg.plan.as_ref().is_some_and(|(s, ..)| *s == sig);
    if !planned && bg.planning != Some(sig) && now - bg.changed >= SETTLE {
        bg.planning = Some(sig);
        let project = Arc::new(state.display_project().clone());
        plan(sig, project, cx);
        return;
    }
    let Some((_, project, plan)) = bg.plan.clone().filter(|(s, ..)| *s == sig) else {
        return;
    };

    let version = store().version();
    if !bg.measuring && version != bg.measured.0 && now - bg.measured.1 >= STATUS_EVERY {
        bg.measuring = true;
        measure(plan.clone(), version, cx);
    }

    let idle = !busy && now - bg.active >= IDLE;
    let waited = bg.retry_at.is_none_or(|at| now >= at);
    let pending = bg.status.as_ref().is_some_and(|s| s.pending());
    if store().background() && idle && waited && pending && bg.worker.is_none() {
        let from = playhead.to_frame(plan.rate);
        bg.worker = Some(Worker::start(project, state.services.clone(), plan, from));
    } else if !store().background() {
        bg.worker = None;
    }
}

/// Plan `project` off the UI thread, then measure what is cached.
fn plan(sig: Signature, project: Arc<Project>, cx: &mut Context<EditorState>) {
    cx.spawn(async move |this, cx| {
        let version = store().version();
        let (project, plan, status) = cx
            .background_executor()
            .spawn(async move {
                let plan = plan_for(&project, sig.globals);
                let status = Status::measure(&plan);
                (project, plan, status)
            })
            .await;
        let _ = this.update(cx, |s, cx| {
            let bg = &mut s.render;
            if bg.planning == Some(sig) {
                bg.planning = None;
            }
            if bg.seen == Some(sig) {
                bg.plan = Some((sig, project, plan));
                bg.status = Some(Arc::new(status));
                bg.measured = (version, Instant::now());
                cx.notify();
            }
        });
    })
    .detach();
}

fn measure(plan: Arc<Plan>, version: u64, cx: &mut Context<EditorState>) {
    cx.spawn(async move |this, cx| {
        let (plan, status) = cx
            .background_executor()
            .spawn(async move {
                let status = Status::measure(&plan);
                (plan, status)
            })
            .await;
        let _ = this.update(cx, |s, cx| {
            let bg = &mut s.render;
            bg.measuring = false;
            bg.measured = (version, Instant::now());
            if bg
                .plan
                .as_ref()
                .is_some_and(|(_, _, p)| Arc::ptr_eq(p, &plan))
            {
                bg.status = Some(Arc::new(status));
                cx.notify();
            }
        });
    })
    .detach();
}
