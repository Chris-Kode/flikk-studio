//! The background render: a thread that composes the frames of a [`Plan`]
//! not on disk yet, from the playhead on and then from the start, with the
//! same compositor as the viewer and the export
//! ([`koo_core::compose::submit_program`]), and writes them to the
//! [`store`]. Frames are pipelined as in playback. A span waits while its
//! media is offline or its subjects' mattes are still being made; person
//! masks are matted, depth maps made and optical-flow frames made in place
//! first, so no
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
use koo_core::speed::Waiting;
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
                // Make optical-flow frames in place, as renders do, so no
                // stand-in blend is kept.
                let mut services = services;
                if let Some(flow) = services.interpolation.take() {
                    services.interpolation = Some(Arc::new(Waiting(flow)));
                }
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
    // A stabilized clip draws unstabilized until its camera path is
    // measured.
    let measured = services.stabilizer.as_ref().is_none_or(|stabilizer| {
        span.stabilized.iter().all(|id| {
            project.timeline.clip(*id).is_some_and(|(_, clip)| {
                koo_core::stabilize::source(project, clip)
                    .is_none_or(|(item, _)| stabilizer.path(item).is_some())
            })
        })
    });
    let depth_made = || {
        let Some(service) = services.depth.as_ref().filter(|d| d.available().is_ok()) else {
            return true;
        };
        span.depth.iter().all(|(clip, range)| {
            depth_source(project, *clip, *range).is_none_or(|(item, r)| {
                let status = service.status(&item, r);
                status.error.is_none() && status.complete()
            })
        })
    };
    if !online || !measured || !depth_made() {
        return false;
    }
    let Some(subjects) = &services.subjects else {
        return true;
    };
    span.subjects.iter().all(|(clip, mask)| {
        let Some((_, clip)) = project.timeline.clip(*clip) else {
            return false;
        };
        let Some(mask) = clip.masks.iter().find(|m| m.id == *mask) else {
            return false;
        };
        if mask.is_person() {
            return compose::person_ref(project, clip, mask).is_some_and(|person| {
                let status = subjects.person_status(person);
                status.error.is_none() && status.tracked >= status.frames
            });
        }
        compose::subject_ref(project, clip, mask)
            .is_some_and(|subject| subjects.status(subject).complete(subject.subject.track))
    })
}

/// Matte the frames of `span`'s person masks, as a render does, so the
/// span can be kept; gives up when `stop` is set or a mask can't be made.
fn complete_people(project: &Project, services: &Services, span: &Span, stop: &AtomicBool) {
    let Some(subjects) = &services.subjects else {
        return;
    };
    for (clip, mask) in &span.subjects {
        let Some((_, clip)) = project.timeline.clip(*clip) else {
            continue;
        };
        let Some(mask) = clip.masks.iter().find(|m| m.id == *mask) else {
            continue;
        };
        let Some(person) = compose::person_ref(project, clip, mask) else {
            continue;
        };
        let status = subjects.person_status(person);
        if status.error.is_some() || status.tracked >= status.frames {
            continue;
        }
        if stop.load(Ordering::Relaxed) || subjects.complete_person(person, &|_| {}, stop).is_err()
        {
            return;
        }
    }
}

/// What `clip` reads depth of over timeline `range`: its media and source
/// range.
fn depth_source(
    project: &Project,
    clip: koo_core::ClipId,
    range: koo_core::TimeRange,
) -> Option<(koo_core::model::MediaItem, koo_core::TimeRange)> {
    let (track, clip) = project.timeline.clip(clip)?;
    let (item, r) = koo_core::depth::source(project, track, clip, range)?;
    Some((item.clone(), r))
}

/// Make the depth maps `span`'s clips read, as a render does, so the span
/// can be kept; gives up when `stop` is set or a map can't be made.
fn complete_depth(project: &Project, services: &Services, span: &Span, stop: &AtomicBool) {
    let Some(service) = services.depth.as_ref().filter(|d| d.available().is_ok()) else {
        return;
    };
    for (clip, range) in &span.depth {
        let Some((item, r)) = depth_source(project, *clip, *range) else {
            continue;
        };
        let status = service.status(&item, r);
        if status.error.is_some() || status.complete() {
            continue;
        }
        if stop.load(Ordering::Relaxed) || service.complete(&item, r, &|_| {}, stop).is_err() {
            return;
        }
    }
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
        complete_people(project, services, span, stop);
        complete_depth(project, services, span, stop);
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
