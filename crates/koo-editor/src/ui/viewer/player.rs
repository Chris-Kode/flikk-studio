//! Real-time video for the program monitor. A background thread follows the
//! playhead and composes each sequence frame there
//! ([`koo_core::compose::program`]) from one playback stream per visible
//! layer ([`Streams`]: reopened at cuts and jumps, decoded and dropped when
//! a clip runs faster than 1×, held when slower), and publishes the newest
//! picture converted for GPUI.

use gpui_kit::RenderImage;
use koo_core::Time;
use koo_core::compose::{self, Streams};
use koo_core::model::Project;
use koo_core::services::{Frame, Services};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Default)]
struct Shared {
    target: AtomicI64,
    stop: AtomicBool,
    /// Newest picture not yet taken by the UI.
    latest: Mutex<Option<Arc<RenderImage>>>,
}

pub struct Player {
    shared: Arc<Shared>,
}

impl Player {
    /// Compose `project` from `from` at `width` pixels wide.
    pub fn start(services: Services, project: Arc<Project>, from: Time, width: u32) -> Self {
        let shared = Arc::new(Shared::default());
        shared.target.store(from.0, Ordering::Relaxed);
        let worker = shared.clone();
        let _ = thread::Builder::new()
            .name("koo-player".into())
            .spawn(move || run(worker, services, project, width));
        Self { shared }
    }

    pub fn set_target(&self, t: Time) {
        self.shared.target.store(t.0, Ordering::Relaxed);
    }

    /// The newest picture since the last call.
    pub fn take(&self) -> Option<Arc<RenderImage>> {
        self.shared.latest.lock().ok()?.take()
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

/// RGBA frame → BGRA `RenderImage`.
pub fn to_image(frame: &Frame) -> Option<Arc<RenderImage>> {
    let mut bgra = frame.rgba.to_vec();
    for px in bgra.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    let buffer = image::RgbaImage::from_raw(frame.width, frame.height, bgra)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}

fn run(shared: Arc<Shared>, services: Services, project: Arc<Project>, width: u32) {
    let rate = project.timeline.settings.rate;
    let size = compose::size_for_width(&project, width);
    let mut streams = Streams::new(services.media.clone(), width, rate);
    let mut shown: Option<i64> = None;
    while !shared.stop.load(Ordering::Relaxed) {
        let frame = Time(shared.target.load(Ordering::Relaxed)).to_frame(rate);
        if shown == Some(frame) {
            thread::sleep(Duration::from_millis(2));
            continue;
        }
        shown = Some(frame);
        let t = Time::from_frame(frame, rate);
        let picture = compose::program(&project, t, size, &services, &mut streams);
        if let Some(image) = to_image(&picture)
            && let Ok(mut latest) = shared.latest.lock()
        {
            *latest = Some(image);
        }
    }
}
