//! Drawing a scene where it is quickest: on the GPU when there is one and
//! it draws, on the CPU otherwise. The CPU's picture ([`Scene::render`] and
//! its kin) is the reference; the GPU's matches it, so a caller never knows
//! which drew a frame.
//!
//! A frame on the GPU takes its turn there (`flikk_os::gpu`) as its caller
//! says: watched, asked for, or one of a listed job's. A frame the GPU
//! fails to draw, for any reason, is drawn on the CPU, and the GPU is left
//! alone for a while before it is tried again.

use super::Scene;
use super::gpu;
use super::render::{Look, Plan};
use crate::services::{ComputeService, Frame};
use flikk_os::backoff::Failures;
use flikk_os::gpu::{Job, Level, Turn, Watching};
use flikk_os::sync::lock;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

/// Draws pictures of scenes, on the GPU when it can. Cheap to clone: clones
/// share their engine and what it last did.
#[derive(Clone)]
pub struct Renderer {
    engine: Option<Arc<dyn ComputeService>>,
    /// When the GPU last failed a frame.
    failures: Arc<Mutex<Failures<()>>>,
}

impl Default for Renderer {
    /// Draws on the CPU.
    fn default() -> Self {
        Self::new(None)
    }
}

impl std::fmt::Debug for Renderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let on = if self.engine.is_some() { "GPU" } else { "CPU" };
        write!(f, "Renderer({on})")
    }
}

/// How a frame on the GPU takes its turn there.
pub(super) enum Turns<'a> {
    /// The user is looking at it: it never waits.
    Watched,
    /// The user asked for it and waits on it; it is not listed.
    Asked,
    /// One unit of a listed job named `label`, opened as the first frame
    /// goes to the GPU; it stops waiting once `cancel` is set.
    Listed {
        job: &'a mut Option<Job>,
        label: &'a str,
        cancel: &'a AtomicBool,
    },
}

/// The cancel of turns that are never cancelled.
static NEVER: AtomicBool = AtomicBool::new(false);

impl Renderer {
    /// One that draws on `engine` when it can, and on the CPU without it.
    pub fn new(engine: Option<Arc<dyn ComputeService>>) -> Self {
        Self {
            engine,
            failures: Arc::default(),
        }
    }

    /// The scene at `t` seconds as `look` shows it, `size` pixels, opaque
    /// RGBA, for work the user asked for and waits on: a still, a preview.
    /// [`Scene::render`], [`Scene::render_outside`] and
    /// [`Scene::render_pass`] say what each look is.
    pub fn render(&self, scene: &Scene, t: f64, size: (u32, u32), look: Look) -> Frame {
        let plan = scene.plan(t, size, look);
        // Never `None`: nothing cancels an asked turn.
        self.draw(&plan, Turns::Asked)
            .unwrap_or_else(|| plan.draw())
    }

    /// [`Renderer::render`] for a picture on screen now, which never waits
    /// for other work on the GPU.
    pub fn render_watched(&self, scene: &Scene, t: f64, size: (u32, u32), look: Look) -> Frame {
        let plan = scene.plan(t, size, look);
        // Never `None`: a watched frame takes no turn.
        self.draw(&plan, Turns::Watched)
            .unwrap_or_else(|| plan.draw())
    }

    /// `plan` drawn, taking its turn on the GPU as `turns` says; `None`
    /// only when a listed job is cancelled while it waits for its turn.
    pub(super) fn draw(&self, plan: &Plan, turns: Turns<'_>) -> Option<Frame> {
        let Some(engine) = self.engine.as_ref().filter(|_| self.trusted()) else {
            return Some(plan.draw());
        };
        // The GPU is held for as long as these are.
        let (mut watching, mut turn): (Option<Watching>, Option<Turn>) = (None, None);
        match turns {
            Turns::Watched => watching = Some(flikk_os::gpu::watching()),
            Turns::Asked => turn = Some(flikk_os::gpu::turn(Level::Asked, &NEVER)?),
            Turns::Listed { job, label, cancel } => {
                let job = job.get_or_insert_with(|| Job::new(label, Level::Asked));
                turn = Some(job.turn(cancel)?);
            }
        }
        // Whatever goes wrong on the GPU, the CPU draws the frame.
        let drawn = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            gpu::draw(engine.as_ref(), plan).map_err(|e| e.0)
        }));
        drop((watching, turn));
        let failed = match drawn {
            Ok(Ok(frame)) => return Some(frame),
            Ok(Err(e)) => e,
            Err(_) => "the GPU's drawing crashed".to_owned(),
        };
        eprintln!("flikk: blockouts are drawn on the CPU for now: {failed}");
        lock(&self.failures).record((), failed);
        Some(plan.draw())
    }

    /// Whether the GPU has not failed a frame lately.
    fn trusted(&self) -> bool {
        lock(&self.failures).recent(&()).is_none()
    }
}
