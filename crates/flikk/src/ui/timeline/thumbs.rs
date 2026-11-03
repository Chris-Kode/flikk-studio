//! Filmstrip thumbnails for video clips, from `MediaService::frame` at a small
//! width. Source times are quantized to a zoom-dependent grid so scrolling
//! reuses tiles; a few requests run at a time; old tiles are evicted.

use flikk_core::model::MediaItem;
use flikk_core::services::MediaService;
use flikk_core::{MediaId, Time};
use gpui_kit::RenderImage;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

pub const THUMB_WIDTH: u32 = 128;
const MAX_IN_FLIGHT: usize = 3;
const MAX_CACHED: usize = 800;

pub type Key = (MediaId, i64);

#[derive(Default)]
pub struct Thumbs {
    /// `None` while queued or in flight, or after a failure.
    images: HashMap<Key, Option<Arc<RenderImage>>>,
    order: VecDeque<Key>,
    queue: VecDeque<(Key, MediaItem, Time)>,
    in_flight: usize,
    /// Evicted images, to free from the sprite atlas on the next render.
    pub retired: Vec<Arc<RenderImage>>,
}

/// Grid step for tiles `tile_secs` apart: a power of two of 1/8 s.
pub fn step(tile_secs: f64) -> Time {
    let mut s = 0.125;
    while s < tile_secs {
        s *= 2.;
    }
    Time::from_secs_f64(s)
}

impl Thumbs {
    /// The tile for `item` near source time `t` on the `step` grid; queues a
    /// fetch on a miss.
    pub fn get(&mut self, item: &MediaItem, t: Time, step: Time) -> Option<Arc<RenderImage>> {
        let q = t.0.div_euclid(step.0.max(1)) * step.0.max(1);
        let key = (item.id, q);
        if let Some(image) = self.images.get(&key) {
            return image.clone();
        }
        self.images.insert(key, None);
        self.order.push_back(key);
        self.queue.push_back((key, item.clone(), Time(q)));
        while self.order.len() > MAX_CACHED {
            if let Some(old) = self.order.pop_front()
                && let Some(Some(image)) = self.images.remove(&old)
            {
                self.retired.push(image);
            }
        }
        None
    }

    /// Fetches to start now, newest first: the latest view matters most.
    pub fn next_requests(&mut self) -> Vec<(Key, MediaItem, Time)> {
        let mut out = Vec::new();
        while self.in_flight < MAX_IN_FLIGHT {
            let Some(req) = self.queue.pop_back() else {
                break;
            };
            if !self.images.contains_key(&req.0) {
                continue;
            }
            self.in_flight += 1;
            out.push(req);
        }
        out
    }

    pub fn finish(&mut self, key: Key, image: Option<Arc<RenderImage>>) {
        self.in_flight = self.in_flight.saturating_sub(1);
        if let Some(slot) = self.images.get_mut(&key) {
            *slot = image;
        }
    }

    /// Drop queued fetches the current view no longer shows.
    pub fn trim_queue(&mut self, keep: usize) {
        while self.queue.len() > keep {
            if let Some((key, _, _)) = self.queue.pop_front() {
                self.images.remove(&key);
                self.order.retain(|k| *k != key);
            }
        }
    }
}

/// Blocking; run on a background thread.
pub fn fetch(media: &dyn MediaService, item: &MediaItem, t: Time) -> Option<Arc<RenderImage>> {
    let frame = media.frame(item, t, THUMB_WIDTH).ok()?;
    crate::ui::viewer::frame_image(&frame)
}
