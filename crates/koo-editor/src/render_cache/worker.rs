//! The background render: a thread that composes the frames of a [`Plan`]
//! not on disk yet, from the playhead on and then from the start, with the
//! same compositor as the viewer and the export
//! ([`koo_core::compose::submit_program`]), and writes them to the
//! [`store`]. Frames are pipelined as in playback. A span waits while its
//! media is offline or its subjects' mattes are still being made, so no
//! frame is kept that live composing would draw differently later.
//! Dropping the worker stops it after the frame in hand; frames are keyed
//! by what they show, so one finished after an edit is simply never asked
//! for.

use super::key::{Key, Plan, Span};
use super::store::store;
use koo_core::Time;
use koo_core::compose::{self, MaskView, Pending, Streams};
use koo_core::model::Project;
use koo_core::services::{PixelOrder, Services};
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

pub struct Worker {
    stop: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
}

impl Worker {
    pub fn start(project: Arc<Project>, services: Services, plan: Arc<Plan>, from: i64) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let (s, d) = (stop.clone(), done.clone());
        let _ = thread::Builder::new()
            .name("koo-render-cache".into())
            .spawn(move || {
                run(&project, &services, &plan, from, &s);
                d.store(true, Ordering::Relaxed);
            });
        Self { stop, done }
    }

    /// Everything it could render is on disk (or it gave up).
    pub fn finished(&self) -> bool {
        self.done.load(Ordering::Relaxed)
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Whether `span` can be drawn as it will finally look.
pub fn ready(project: &Project, services: &Services, span: &Span) -> bool {
    let online = span.media.iter().all(|id| {
        project
            .media(*id)
            .is_some_and(|m| m.path.exists() || m.proxy.as_ref().is_some_and(|p| p.exists()))
    });
    let Some(subjects) = &services.subjects else {
        return online;
    };
    online
        && span.subjects.iter().all(|(clip, mask)| {
            let Some((_, clip)) = project.timeline.clip(*clip) else {
                return false;
            };
            let Some(mask) = clip.masks.iter().find(|m| m.id == *mask) else {
                return false;
            };
            compose::subject_ref(project, clip, mask)
                .is_some_and(|subject| subjects.status(subject).complete(subject.subject.track))
        })
}

/// The frames of `span` from `from` on, then those before.
fn order(plan: &Plan, from: i64) -> Vec<(usize, Range<i64>)> {
    let mut later = Vec::new();
    let mut earlier = Vec::new();
    for (i, span) in plan.spans.iter().enumerate() {
        let f = &span.frames;
        if f.end <= from {
            earlier.push((i, f.clone()));
        } else if f.start >= from {
            later.push((i, f.clone()));
        } else {
            later.insert(0, (i, from..f.end));
            earlier.push((i, f.start..from));
        }
    }
    later.sort_by_key(|(_, r)| r.start);
    later.extend(earlier);
    later
}

fn run(project: &Project, services: &Services, plan: &Plan, from: i64, stop: &AtomicBool) {
    let rate = plan.rate;
    let size = plan.size;
    let mut streams = Streams::new(services.media.clone(), size.0, rate);
    let mut flight: Option<(Key, Pending<'_>)> = None;
    let land = |(key, pending): (Key, Pending<'_>)| {
        let frame = pending.wait();
        if (frame.width, frame.height) == size {
            store().write(&key, frame.width, frame.height, &frame.rgba);
        }
    };
    for (i, frames) in order(plan, from) {
        let span = &plan.spans[i];
        if !ready(project, services, span) {
            continue;
        }
        for frame in frames {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            let key = span.key(frame);
            if store().contains(&key) {
                continue;
            }
            let pending = compose::submit_program(
                project,
                Time::from_frame(frame, rate),
                size,
                services,
                &mut streams,
                PixelOrder::Bgra,
                MaskView::Cut,
            );
            if let Some(previous) = flight.replace((key, pending)) {
                land(previous);
            }
        }
    }
    if let Some(last) = flight
        && !stop.load(Ordering::Relaxed)
    {
        land(last);
    }
}
