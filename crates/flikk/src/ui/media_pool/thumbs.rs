//! Pool thumbnails: one frame at 10 % of the item, decoded off-thread and
//! cached by fingerprint for the session.

use super::MediaPoolPanel;
use crate::state::EditorState;
use flikk_core::Time;
use flikk_core::model::{MediaItem, MediaKind};
use flikk_core::services::Frame;
use gpui_kit::*;
use std::collections::HashMap;
use std::sync::Arc;

const WIDTH: u32 = 320;

enum Thumb {
    Pending,
    Ready(Arc<Image>),
    Missing,
}

#[derive(Default)]
pub struct Thumbs(HashMap<String, Thumb>);

impl Thumbs {
    /// The cached image, starting a decode on first request.
    pub fn get(
        &mut self,
        item: &MediaItem,
        state: &Entity<EditorState>,
        cx: &mut Context<MediaPoolPanel>,
    ) -> Option<Arc<Image>> {
        let key = key(item);
        match self.0.get(&key) {
            Some(Thumb::Ready(image)) => return Some(image.clone()),
            Some(_) => return None,
            None => {}
        }
        let media = state.read(cx).services.media.clone();
        let Some(media) = media.filter(|_| item.kind != MediaKind::Audio) else {
            // Retry once a service is wired.
            return None;
        };
        self.0.insert(key.clone(), Thumb::Pending);
        let item = item.clone();
        let at = item
            .info
            .as_ref()
            .map(|i| Time(i.duration.0 / 10))
            .unwrap_or(Time::ZERO);
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
                this.thumbs.0.insert(key, thumb);
                cx.notify();
            })
            .ok();
        })
        .detach();
        None
    }
}

fn key(item: &MediaItem) -> String {
    match &item.fingerprint {
        Some(fp) => fp.0.clone(),
        None => item.id.0.to_string(),
    }
}

/// RGBA8 → 32-bit top-down BMP, which GPUI decodes without extra crates.
fn bmp(frame: &Frame) -> Vec<u8> {
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
