//! Pool thumbnails: one frame at 10 % of the item, the frames a pointer
//! skims through, and a sound's peaks; read off-thread and cached by
//! fingerprint and path for the session.

use super::MediaPoolPanel;
use crate::state::EditorState;
use flikk_core::model::{MediaItem, MediaKind};
use flikk_core::services::Frame;
use flikk_core::{Time, TimeRange};
use gpui_kit::*;
use std::collections::HashMap;
use std::sync::Arc;

const WIDTH: u32 = 320;

/// Frames a thumbnail can skim through, evenly over the item.
pub const SKIM_SLOTS: u32 = 40;
/// Peaks across an audio tile.
const PEAKS: usize = 64;

enum Thumb {
    Pending,
    Ready(Arc<Image>),
    Missing,
}

/// Thumbnails, skimmed frames and audio tiles' peaks, by item.
#[derive(Default)]
pub struct Thumbs {
    frames: HashMap<String, Thumb>,
    /// `None` while being read or when there are none.
    peaks: HashMap<String, Option<Arc<Vec<f32>>>>,
}

impl Thumbs {
    /// The item's poster frame, at 10 %, starting a decode on first
    /// request.
    pub fn get(
        &mut self,
        item: &MediaItem,
        state: &Entity<EditorState>,
        cx: &mut Context<MediaPoolPanel>,
    ) -> Option<Arc<Image>> {
        let at = item
            .info
            .as_ref()
            .map(|i| Time(i.duration.0 / 10))
            .unwrap_or(Time::ZERO);
        self.frame(key(item), item, at, state, cx)
    }

    /// The frame at `slot` of [`SKIM_SLOTS`] while the pointer skims a
    /// video's tile; until it decodes, the nearest one that has.
    pub fn skim(
        &mut self,
        item: &MediaItem,
        slot: u32,
        state: &Entity<EditorState>,
        cx: &mut Context<MediaPoolPanel>,
    ) -> Option<Arc<Image>> {
        let duration = item.info.as_ref()?.duration;
        let slot = slot.min(SKIM_SLOTS - 1);
        let at = Time((duration.0 as f64 * (slot as f64 + 0.5) / SKIM_SLOTS as f64) as i64);
        let base = key(item);
        if let Some(image) = self.frame(format!("{base}#{slot}"), item, at, state, cx) {
            return Some(image);
        }
        (1..SKIM_SLOTS).find_map(|d| {
            [
                slot.checked_sub(d),
                Some(slot + d).filter(|s| *s < SKIM_SLOTS),
            ]
            .into_iter()
            .flatten()
            .find_map(|s| match self.frames.get(&format!("{base}#{s}")) {
                Some(Thumb::Ready(image)) => Some(image.clone()),
                _ => None,
            })
        })
    }

    /// A sound's peaks across its whole length, 0..1, read on first
    /// request.
    pub fn peaks(
        &mut self,
        item: &MediaItem,
        state: &Entity<EditorState>,
        cx: &mut Context<MediaPoolPanel>,
    ) -> Option<Arc<Vec<f32>>> {
        let key = key(item);
        if let Some(peaks) = self.peaks.get(&key) {
            return peaks.clone();
        }
        let media = state.read(cx).services.media.clone()?;
        let info = item.info.as_ref()?;
        let stream = info.audio.first()?.index;
        let range = TimeRange::new(Time::ZERO, info.duration);
        self.peaks.insert(key.clone(), None);
        let item = item.clone();
        cx.spawn(async move |this, cx| {
            let peaks = cx
                .background_executor()
                .spawn(async move { media.waveform(&item, stream, range, PEAKS).ok() })
                .await;
            this.update(cx, |this, cx| {
                this.thumbs.peaks.insert(key, peaks.map(Arc::new));
                cx.notify();
            })
            .ok();
        })
        .detach();
        None
    }

    fn frame(
        &mut self,
        key: String,
        item: &MediaItem,
        at: Time,
        state: &Entity<EditorState>,
        cx: &mut Context<MediaPoolPanel>,
    ) -> Option<Arc<Image>> {
        match self.frames.get(&key) {
            Some(Thumb::Ready(image)) => return Some(image.clone()),
            Some(_) => return None,
            None => {}
        }
        let media = state.read(cx).services.media.clone();
        let Some(media) = media.filter(|_| item.kind != MediaKind::Audio) else {
            // Retry once a service is wired.
            return None;
        };
        self.frames.insert(key.clone(), Thumb::Pending);
        let item = item.clone();
        cx.spawn(async move |this, cx| {
            let image = cx
                .background_executor()
                .spawn(async move { media.frame(&item, at, WIDTH).ok().map(|f| bmp(&f)) })
                .await;
            this.update(cx, |this, cx| {
                let thumb = match image {
                    Some(bytes) => {
                        Thumb::Ready(Arc::new(Image::from_bytes(ImageFormat::Bmp, bytes)))
                    }
                    None => Thumb::Missing,
                };
                this.thumbs.frames.insert(key, thumb);
                cx.notify();
            })
            .ok();
        })
        .detach();
        None
    }
}

/// By fingerprint, so copies share, and by path, so an item relinked
/// after its frame or peaks failed reads them again.
fn key(item: &MediaItem) -> String {
    let base = match &item.fingerprint {
        Some(fp) => fp.0.clone(),
        None => item.id.0.to_string(),
    };
    format!("{base}|{}", item.path.display())
}

/// RGBA8 → 32-bit top-down BMP, which GPUI decodes without extra crates.
pub(crate) fn bmp(frame: &Frame) -> Vec<u8> {
    let (w, h) = (frame.width, frame.height);
    let data = w as usize * h as usize * 4;
    let mut out = Vec::with_capacity(54 + data);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(54 + data as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(-(h as i32)).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(data as u32).to_le_bytes());
    out.extend_from_slice(&[0; 16]);
    for px in frame.rgba.as_chunks::<4>().0.iter().take(data / 4) {
        out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
    }
    out
}
