//! Real-time video for the program monitor. A background thread follows the
//! playhead, keeps one `MediaService::playback` stream open for the topmost
//! clip, reopens it at cuts or jumps, and publishes the frame for the current
//! time, converted for GPUI off the UI thread.

use gpui_kit::RenderImage;
use koo_core::model::{Clip, MediaItem, MediaKind, Project, Track, TrackKind};
use koo_core::services::{Frame, FrameStream, MediaService};
use koo_core::{ClipId, Time};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Clone)]
pub struct Picture {
    pub clip: ClipId,
    pub image: Arc<RenderImage>,
}

#[derive(Default)]
struct Shared {
    target: AtomicI64,
    stop: AtomicBool,
    /// Newest decoded picture not yet taken by the UI; `Some(None)` = black.
    latest: Mutex<Option<Option<Picture>>>,
}

pub struct Player {
    shared: Arc<Shared>,
}

impl Player {
    pub fn start(
        media: Arc<dyn MediaService>,
        project: Arc<Project>,
        from: Time,
        max_width: u32,
    ) -> Self {
        let shared = Arc::new(Shared::default());
        shared.target.store(from.0, Ordering::Relaxed);
        let worker = shared.clone();
        let _ = thread::Builder::new()
            .name("koo-player".into())
            .spawn(move || run(worker, media, project, max_width));
        Self { shared }
    }

    pub fn set_target(&self, t: Time) {
        self.shared.target.store(t.0, Ordering::Relaxed);
    }

    /// The newest picture since the last call: `Some(None)` means the
    /// playhead is over a gap.
    pub fn take(&self) -> Option<Option<Picture>> {
        self.shared.latest.lock().ok()?.take()
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

/// The topmost visible media clip at `t`: tracks are stored top to bottom.
pub fn top_clip(project: &Project, t: Time) -> Option<(&Track, &Clip, &MediaItem)> {
    project
        .timeline
        .tracks
        .iter()
        .filter(|tr| matches!(tr.kind, TrackKind::Video | TrackKind::Overlay) && !tr.mix.mute)
        .find_map(|tr| {
            let clip = tr.clip_at(t)?;
            let item = project.media(clip.media()?)?;
            (item.kind != MediaKind::Audio).then_some((tr, clip, item))
        })
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

struct Open {
    clip: ClipId,
    stream: Box<dyn FrameStream>,
    /// Source time of the last frame read.
    position: Option<Time>,
}

fn run(shared: Arc<Shared>, media: Arc<dyn MediaService>, project: Arc<Project>, max_width: u32) {
    let rate = project.timeline.settings.rate;
    let frame = rate.frame_duration();
    let mut open: Option<Open> = None;
    let mut shown: Option<(ClipId, Time)> = None;
    let publish = |picture: Option<Picture>| {
        if let Ok(mut latest) = shared.latest.lock() {
            *latest = Some(picture);
        }
    };
    while !shared.stop.load(Ordering::Relaxed) {
        let t = Time(shared.target.load(Ordering::Relaxed));
        let Some((_, clip, item)) = top_clip(&project, t) else {
            if shown.take().is_some() {
                publish(None);
            }
            open = None;
            thread::sleep(Duration::from_millis(4));
            continue;
        };
        let Some(source) = clip.to_source(t) else {
            thread::sleep(Duration::from_millis(4));
            continue;
        };
        let reopen = match &open {
            Some(o) if o.clip == clip.id => o
                .position
                .is_some_and(|p| source + frame < p || source - p > Time::from_secs_f64(1.5)),
            _ => true,
        };
        if reopen {
            open = match media.playback(item, source, max_width) {
                Ok(stream) => Some(Open {
                    clip: clip.id,
                    stream,
                    position: None,
                }),
                Err(_) => None,
            };
        }
        let Some(o) = open.as_mut() else {
            thread::sleep(Duration::from_millis(20));
            continue;
        };
        // Decode up to the frame on screen now; late frames are skipped.
        let mut newest = None;
        while o.position.is_none_or(|p| p + frame <= source) {
            match o.stream.next_frame() {
                Ok(Some((at, f))) => {
                    o.position = Some(at);
                    newest = Some(f);
                }
                _ => break,
            }
        }
        if let (Some(f), Some(at)) = (newest, o.position)
            && shown != Some((clip.id, at))
            && let Some(image) = to_image(&f)
        {
            shown = Some((clip.id, at));
            publish(Some(Picture {
                clip: clip.id,
                image,
            }));
        }
        thread::sleep(Duration::from_millis(3));
    }
}
