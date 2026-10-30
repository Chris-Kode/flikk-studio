//! Composed program frames kept for the viewer: paused frames, full-quality
//! playback frames and frames rendered ahead while paused. Scrubbing back
//! over them, or playing them again, needs no composing. Frames belong to
//! one [`PlayKey`] (project version, previewed change set, width); a frame
//! for another key clears the rest.

use super::PlayKey;
use gpui_kit::RenderImage;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

/// Memory the kept frames may use; the oldest go first.
const MAX_BYTES: usize = 768 << 20;

pub type SharedCache = Arc<Mutex<FrameCache>>;

#[derive(Default)]
pub struct FrameCache {
    key: Option<PlayKey>,
    frames: HashMap<i64, (Arc<RenderImage>, usize)>,
    /// Frame indices, least recently used first.
    order: VecDeque<i64>,
    bytes: usize,
}

impl FrameCache {
    pub fn shared() -> SharedCache {
        Arc::new(Mutex::new(Self::default()))
    }

    pub fn get(&mut self, key: PlayKey, frame: i64) -> Option<Arc<RenderImage>> {
        if self.key != Some(key) {
            return None;
        }
        let (image, _) = self.frames.get(&frame)?;
        let image = image.clone();
        self.touch(frame);
        Some(image)
    }

    /// Let every frame go: something they show changed outside the key
    /// (a subject's matte was made).
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn contains(&self, key: PlayKey, frame: i64) -> bool {
        self.key == Some(key) && self.frames.contains_key(&frame)
    }

    /// Keep `image` (`bytes` large) as `frame` of `key`.
    pub fn insert(&mut self, key: PlayKey, frame: i64, image: Arc<RenderImage>, bytes: usize) {
        if self.key != Some(key) {
            *self = Self {
                key: Some(key),
                ..Self::default()
            };
        }
        if let Some((_, old)) = self.frames.insert(frame, (image, bytes)) {
            self.bytes -= old;
            self.touch(frame);
        } else {
            self.order.push_back(frame);
        }
        self.bytes += bytes;
        while self.bytes > MAX_BYTES
            && let Some(oldest) = self.order.pop_front()
        {
            if let Some((_, b)) = self.frames.remove(&oldest) {
                self.bytes -= b;
            }
        }
    }

    fn touch(&mut self, frame: i64) {
        if let Some(i) = self.order.iter().position(|f| *f == frame) {
            self.order.remove(i);
        }
        self.order.push_back(frame);
    }
}
