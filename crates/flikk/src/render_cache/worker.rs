//! The background render: a thread that composes the frames of a [`Plan`]
//! not on disk yet, from the playhead on and then from the start, with the
//! same compositor as the viewer and the export
//! ([`flikk_core::compose::submit_program`]), and writes them to the
//! [`store`]. Frames are pipelined as in playback. A span waits while its
//! media is offline or its subjects' mattes are still being made; person
//! masks are matted, depth maps made and optical-flow frames made in place
//! first, so no
//! frame is kept that live composing would draw differently later.
//! Dropping the worker stops it after the frame in hand; frames are keyed
//! by what they show, so one finished after an edit is simply never asked
//! for. Each frame composed is a background GPU turn
//! ([`flikk_os::gpu`]), under a job listed from the first frame that
//! needs composing.
//!
//! The first frames of each span are timed, one at a time: a span that
//! composes well within a frame's time is [`cheap`], drawn live as fast as
//! read from disk, so the rest of it is not kept.

use super::key::{Key, Plan, Span};
use super::store::store;
use flikk_core::Time;
use flikk_core::compose::{self, MaskView, Pending, Streams};
use flikk_core::model::Project;
use flikk_core::services::{Frame, PixelOrder, Services};
use flikk_core::speed::Waiting;
use flikk_os::gpu;
use std::collections::HashSet;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Frames of a span timed before it is judged; the first ones open and
/// seek the media, so the median is taken.
const SAMPLE: usize = 9;
/// A span whose frames compose in less than this share of a frame's
/// time is cheap.
const CHEAP_SHARE: f64 = 0.25;

/// Spans found cheap this session, by what they draw.
static CHEAP: Mutex<Option<HashSet<[u8; 32]>>> = Mutex::new(None);

/// Whether `span` was found cheap to compose: it is drawn live.
pub fn cheap(span: &Span) -> bool {
    CHEAP
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|c| c.contains(&span.id()))
}

fn set_cheap(span: &Span) {
    CHEAP
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_default()
        .insert(span.id());
}

pub struct Worker {
    stop: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
}

impl Worker {
    /// `None` when the thread could not be started.
    pub fn start(
        project: Arc<Project>,
        services: Services,
        plan: Arc<Plan>,
        from: i64,
    ) -> Option<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let (s, d) = (stop.clone(), done.clone());
        thread::Builder::new()
            .name("flikk-render-cache".into())
            .spawn(move || {
                // Make optical-flow frames in place, as renders do, so no
                // stand-in blend is kept.
                let mut services = services;
                if let Some(flow) = services.interpolation.take() {
                    services.interpolation = Some(Arc::new(Waiting(flow)));
                }
                run(&project, &services, &plan, from, &s);
                d.store(true, Ordering::Relaxed);
            })
            .ok()?;
        Some(Self { stop, done })
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
                flikk_core::stabilize::source(project, clip)
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
    clip: flikk_core::ClipId,
    range: flikk_core::TimeRange,
) -> Option<(flikk_core::model::MediaItem, flikk_core::TimeRange)> {
    let (track, clip) = project.timeline.clip(clip)?;
    let (item, r) = flikk_core::depth::source(project, track, clip, range)?;
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

/// Frames composed, waiting to be encoded and written.
const WRITING: usize = 2;

/// Compose and keep the frames, encoding and writing them on a thread of
/// their own, as that takes about as long as composing.
fn run(project: &Project, services: &Services, plan: &Plan, from: i64, stop: &AtomicBool) {
    let size = plan.size;
    thread::scope(|scope| {
        let (tx, rx) = std::sync::mpsc::sync_channel::<(Key, Frame)>(WRITING);
        scope.spawn(move || {
            for (key, frame) in rx {
                store().write(&key, frame.width, frame.height, &frame.rgba);
            }
        });
        let keep = move |key: Key, frame: Frame| {
            if (frame.width, frame.height) == size {
                let _ = tx.send((key, frame));
            }
        };
        compose_all(project, services, plan, from, stop, &keep);
    });
}

fn compose_all(
    project: &Project,
    services: &Services,
    plan: &Plan,
    from: i64,
    stop: &AtomicBool,
    keep: &dyn Fn(Key, Frame),
) {
    let rate = plan.rate;
    let size = plan.size;
    let mut streams = Streams::new(services.media.clone(), size.0, rate);
    let mut flight: Option<(Key, Pending<'_>)> = None;
    let land = |(key, pending): (Key, Pending<'_>)| keep(key, pending.wait());
    let budget = Duration::from_secs_f64(rate.frame_duration().as_secs_f64() * CHEAP_SHARE);
    let order = order(plan, from);
    let total = order
        .iter()
        .map(|(_, f)| f.end - f.start)
        .sum::<i64>()
        .max(1);
    // Frames of the spans before this one, composed or not.
    let mut passed = 0;
    // Opened once a frame needs composing, so a pass over frames all on
    // disk doesn't flash in the list.
    let mut job: Option<gpu::Job> = None;
    for (i, frames) in order {
        let span = &plan.spans[i];
        // Where this span's frames start among all of them.
        let offset = passed - frames.start;
        passed += frames.end - frames.start;
        if cheap(span) {
            continue;
        }
        complete_people(project, services, span, stop);
        complete_depth(project, services, span, stop);
        if !ready(project, services, span) {
            continue;
        }
        let mut timed: Vec<Duration> = Vec::new();
        for frame in frames {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            let key = span.key(frame);
            if store().contains(&key) {
                continue;
            }
            let job =
                job.get_or_insert_with(|| gpu::Job::new("Render cache", gpu::Level::Background));
            let Some(_turn) = job.turn(stop) else {
                return;
            };
            job.progress((offset + frame) as f32 / total as f32);
            let submit = |streams: &mut Streams| {
                compose::submit_program(
                    project,
                    Time::from_frame(frame, rate),
                    size,
                    services,
                    streams,
                    PixelOrder::Bgra,
                    MaskView::Cut,
                )
            };
            if timed.len() < SAMPLE {
                // By itself, so its time is its own.
                if let Some(previous) = flight.take() {
                    land(previous);
                }
                let start = Instant::now();
                let made = submit(&mut streams).wait();
                timed.push(start.elapsed());
                keep(key, made);
                if timed.len() == SAMPLE {
                    timed.sort_unstable();
                    if timed[SAMPLE / 2] < budget {
                        set_cheap(span);
                        break;
                    }
                }
                continue;
            }
            if let Some(previous) = flight.replace((key, submit(&mut streams))) {
                land(previous);
            }
        }
    }
    if let Some(last) = flight
        && let Some(_turn) = job.as_ref().and_then(|job| job.turn(stop))
    {
        land(last);
    }
}
