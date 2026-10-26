//! Real-time video for the program monitor. A background thread follows the
//! playhead, keeps one `MediaService::playback` stream open per visible
//! layer, reopens them at cuts or jumps, and publishes the layers for the
//! current time, converted for GPUI off the UI thread.

use super::grade;
use gpui_kit::RenderImage;
use koo_core::model::{Clip, ClipContent, Grade, MediaItem, MediaKind, Project, TrackKind};
use koo_core::services::{Frame, FrameStream, MediaService};
use koo_core::{ClipId, MediaId, Time};
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// One composited layer, ready to paint.
#[derive(Clone)]
pub struct Picture {
    pub clip: ClipId,
    pub image: Arc<RenderImage>,
    /// Width over height of the source before rotation, for the fit.
    pub aspect: f32,
    /// Growth of the image from rotation, (rotated ÷ source) per axis.
    pub grown: (f32, f32),
}

/// A picture clip under the playhead and the source frame it shows.
pub struct Layer<'a> {
    pub clip: &'a Clip,
    pub item: Cow<'a, MediaItem>,
    pub source: Time,
}

/// Grade, rotation and opacity baked into a layer's pixels.
#[derive(Clone, PartialEq)]
pub struct Look {
    pub grade: Grade,
    /// The timeline grade, after the clip's; neutral for overlay assets.
    pub timeline: Grade,
    pub rotation: f32,
    pub opacity: f32,
}

impl Look {
    pub fn of(clip: &Clip, t: Time, project: &Project) -> Self {
        let tf = clip.transform_at(t - clip.start);
        let timeline = match clip.content {
            ClipContent::Media { .. } => project.timeline.grade.clone(),
            _ => Grade::default(),
        };
        Self {
            grade: clip.grade.clone(),
            timeline,
            rotation: tf.rotation_deg,
            opacity: tf.opacity.clamp(0., 1.),
        }
    }
}

/// Picture layers at `t`, bottom-up, as the render composites them: clips
/// on unmuted video and overlay tracks, media and overlay assets.
pub fn layers_at(project: &Project, t: Time) -> Vec<Layer<'_>> {
    project
        .timeline
        .tracks
        .iter()
        .rev()
        .filter(|tr| matches!(tr.kind, TrackKind::Video | TrackKind::Overlay) && !tr.mix.mute)
        .filter_map(|tr| tr.clip_at(t))
        .filter_map(|clip| match &clip.content {
            ClipContent::Media { media, .. } => {
                let item = project.media(*media)?;
                if item.kind == MediaKind::Audio {
                    return None;
                }
                Some(Layer {
                    clip,
                    item: Cow::Borrowed(item),
                    source: clip.to_source(t)?,
                })
            }
            ClipContent::Overlay { asset, .. } => Some(Layer {
                clip,
                item: Cow::Owned(overlay_item(clip.id, asset)),
                source: t - clip.start,
            }),
            _ => None,
        })
        .collect()
}

/// A pool-less item for an overlay asset, keyed by its clip.
fn overlay_item(clip: ClipId, asset: &Path) -> MediaItem {
    let ext = asset
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let still = matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "webp" | "bmp" | "tif" | "tiff" | "tga"
    );
    MediaItem {
        id: MediaId(clip.0),
        path: asset.to_path_buf(),
        kind: if still {
            MediaKind::Image
        } else {
            MediaKind::Video
        },
        fingerprint: None,
        info: None,
        bin: None,
        tags: Vec::new(),
        proxy: None,
    }
}

#[derive(Default)]
struct Shared {
    target: AtomicI64,
    stop: AtomicBool,
    /// Newest layers not yet taken by the UI; empty = black.
    latest: Mutex<Option<Vec<Picture>>>,
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

    /// The newest layers since the last call.
    pub fn take(&self) -> Option<Vec<Picture>> {
        self.shared.latest.lock().ok()?.take()
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

/// RGBA frame → BGRA `RenderImage`, graded when `grade` is given.
pub fn to_image(frame: &Frame, grade: Option<&Grade>) -> Option<Arc<RenderImage>> {
    let mut bgra = frame.rgba.to_vec();
    if let Some(g) = grade {
        grade::apply(&mut bgra, g);
    }
    for px in bgra.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    let buffer = image::RgbaImage::from_raw(frame.width, frame.height, bgra)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}

/// A decoded frame with `look` applied, as a layer of `clip`.
pub fn picture(clip: ClipId, frame: &Frame, look: &Look) -> Option<Picture> {
    let (w, h) = (frame.width.max(1), frame.height.max(1));
    let mut rgba = frame.rgba.to_vec();
    grade::apply(&mut rgba, &look.grade);
    grade::apply(&mut rgba, &look.timeline);
    if look.opacity < 1. {
        for px in rgba.as_chunks_mut::<4>().0 {
            px[3] = (px[3] as f32 * look.opacity).round() as u8;
        }
    }
    let (rgba, rw, rh) = if look.rotation.abs() > 1e-3 {
        rotate(&rgba, w, h, look.rotation)
    } else {
        (rgba, w, h)
    };
    let mut bgra = rgba;
    for px in bgra.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    let buffer = image::RgbaImage::from_raw(rw, rh, bgra)?;
    Some(Picture {
        clip,
        image: Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])),
        aspect: w as f32 / h as f32,
        grown: (rw as f32 / w as f32, rh as f32 / h as f32),
    })
}

/// Rotate clockwise by `deg` onto a transparent canvas just large enough,
/// like the render's `rotate=…:c=none`.
fn rotate(rgba: &[u8], w: u32, h: u32, deg: f32) -> (Vec<u8>, u32, u32) {
    let (s, c) = deg.to_radians().sin_cos();
    let (wf, hf) = (w as f32, h as f32);
    let rw = (wf * c.abs() + hf * s.abs()).ceil().max(1.) as u32;
    let rh = (wf * s.abs() + hf * c.abs()).ceil().max(1.) as u32;
    let mut out = vec![0u8; (rw * rh * 4) as usize];
    let (cx, cy) = (rw as f32 / 2., rh as f32 / 2.);
    for y in 0..rh {
        let dy = y as f32 + 0.5 - cy;
        for x in 0..rw {
            let dx = x as f32 + 0.5 - cx;
            let sx = dx * c + dy * s + wf / 2.;
            let sy = -dx * s + dy * c + hf / 2.;
            if sx < 0. || sy < 0. || sx >= wf || sy >= hf {
                continue;
            }
            let from = ((sy as u32 * w + sx as u32) * 4) as usize;
            let to = ((y * rw + x) * 4) as usize;
            out[to..to + 4].copy_from_slice(&rgba[from..from + 4]);
        }
    }
    (out, rw, rh)
}

struct Open {
    stream: Box<dyn FrameStream>,
    /// Source time of the last frame read.
    position: Option<Time>,
    /// Last frame read, kept so an animated look can be re-applied.
    frame: Option<Frame>,
    /// Look and picture last published for this layer.
    shown: Option<(Look, Picture)>,
}

fn run(shared: Arc<Shared>, media: Arc<dyn MediaService>, project: Arc<Project>, max_width: u32) {
    let rate = project.timeline.settings.rate;
    let frame = rate.frame_duration();
    let mut open: HashMap<ClipId, Open> = HashMap::new();
    let mut published: Option<Vec<ClipId>> = None;
    while !shared.stop.load(Ordering::Relaxed) {
        let t = Time(shared.target.load(Ordering::Relaxed));
        let layers = layers_at(&project, t);
        open.retain(|id, _| layers.iter().any(|l| l.clip.id == *id));
        let mut changed = false;
        for layer in &layers {
            let clip = layer.clip;
            let source = layer.source;
            // Stills decode once; streams reopen when the playhead jumps.
            let still = layer.item.kind == MediaKind::Image;
            let reopen = open.get(&clip.id).is_none_or(|o| {
                !still
                    && o.position.is_some_and(|p| {
                        source + frame < p || source - p > Time::from_secs_f64(1.5)
                    })
            });
            if reopen {
                match media.playback(&layer.item, source, max_width) {
                    Ok(stream) => {
                        open.insert(
                            clip.id,
                            Open {
                                stream,
                                position: None,
                                frame: None,
                                shown: None,
                            },
                        );
                    }
                    Err(_) => {
                        open.remove(&clip.id);
                        continue;
                    }
                }
            }
            let Some(o) = open.get_mut(&clip.id) else {
                continue;
            };
            // Decode up to the frame on screen now; late frames are skipped.
            let mut fresh = false;
            while o.position.is_none_or(|p| p + frame <= source) {
                match o.stream.next_frame() {
                    Ok(Some((at, f))) => {
                        o.position = Some(at);
                        o.frame = Some(f);
                        fresh = true;
                    }
                    _ => break,
                }
            }
            let look = Look::of(clip, t, &project);
            let stale = fresh || o.shown.as_ref().is_none_or(|(l, _)| *l != look);
            if let (true, Some(f)) = (stale, &o.frame)
                && let Some(p) = picture(clip.id, f, &look)
            {
                o.shown = Some((look, p));
                changed = true;
            }
        }
        let pictures: Vec<Picture> = layers
            .iter()
            .filter_map(|l| open.get(&l.clip.id)?.shown.as_ref().map(|(_, p)| p.clone()))
            .collect();
        let ids: Vec<ClipId> = pictures.iter().map(|p| p.clip).collect();
        if changed || published.as_ref() != Some(&ids) {
            published = Some(ids);
            if let Ok(mut latest) = shared.latest.lock() {
                *latest = Some(pictures);
            }
        }
        thread::sleep(Duration::from_millis(if layers.is_empty() { 4 } else { 3 }));
    }
}
