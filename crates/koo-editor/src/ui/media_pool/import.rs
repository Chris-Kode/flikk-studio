//! Import: probe and fingerprint off-thread, add with `ImportMedia`, then
//! make the playback proxy (PRD M1, M4). Analysis waits for the user.

use super::MediaPoolPanel;
use gpui_kit::*;
use koo_core::command::Command;
use koo_core::model::{MediaItem, MediaKind};
use koo_core::services::MediaService;
use koo_core::{BinId, MediaId};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const VIDEO: &[&str] = &[
    "mp4", "mov", "m4v", "mkv", "webm", "avi", "mxf", "mts", "m2ts", "wmv", "flv",
];
const AUDIO: &[&str] = &[
    "wav", "mp3", "aac", "m4a", "flac", "ogg", "opus", "aif", "aiff", "wma",
];
const IMAGE: &[&str] = &[
    "png", "jpg", "jpeg", "webp", "tif", "tiff", "bmp", "gif", "heic",
];

impl MediaPoolPanel {
    pub(super) fn import(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let media = state.services.media.clone();
        let known: Vec<PathBuf> = state
            .bus
            .project()
            .media
            .iter()
            .map(|m| m.path.clone())
            .collect();
        let bin = self.bin;
        cx.spawn(async move |this, cx| {
            let items = cx
                .background_executor()
                .spawn(async move {
                    expand(paths)
                        .into_iter()
                        .filter(|p| !known.contains(p))
                        .filter_map(|p| build_item(p, bin, media.as_deref()))
                        .collect::<Vec<_>>()
                })
                .await;
            this.update(cx, |this, cx| {
                for item in items {
                    this.add(item, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn add(&mut self, item: MediaItem, cx: &mut Context<Self>) {
        let services = self.state.read(cx).services.clone();
        let applied = self.state.update(cx, |s, cx| {
            s.execute(
                Command::ImportMedia {
                    media: item.clone(),
                },
                cx,
            )
        });
        if applied.is_none() {
            return;
        }
        self.selected = Some(item.id);
        if let Some(media) = services.media
            && item.kind == MediaKind::Video
            && item.proxy.is_none()
        {
            self.make_proxy(item, media, cx);
        }
    }

    fn make_proxy(
        &mut self,
        item: MediaItem,
        media: Arc<dyn MediaService>,
        cx: &mut Context<Self>,
    ) {
        let id = item.id;
        let progress = self.proxies.clone();
        progress.lock().unwrap().insert(id, 0.0);
        cx.spawn(async move |this, cx| {
            let report = progress.clone();
            let path = cx
                .background_executor()
                .spawn(async move {
                    media
                        .ensure_proxy(&item, &|f| {
                            report.lock().unwrap().insert(id, f);
                        })
                        .ok()
                })
                .await;
            progress.lock().unwrap().remove(&id);
            this.update(cx, |this, cx| {
                if let Some(path) = path {
                    this.set_proxy(id, path, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn set_proxy(&mut self, id: MediaId, path: PathBuf, cx: &mut Context<Self>) {
        let Some(mut item) = self.state.read(cx).bus.project().media(id).cloned() else {
            return;
        };
        item.proxy = Some(path);
        self.update_media(item, cx);
    }
}

/// Dropped folders contribute the media files directly inside them.
fn expand(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for p in paths {
        if p.is_dir() {
            let mut inner: Vec<PathBuf> = std::fs::read_dir(&p)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| kind_of(p).is_some())
                .collect();
            inner.sort();
            out.extend(inner);
        } else if kind_of(&p).is_some() {
            out.push(p);
        }
    }
    out
}

fn kind_of(path: &Path) -> Option<MediaKind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let ext = ext.as_str();
    if VIDEO.contains(&ext) {
        Some(MediaKind::Video)
    } else if AUDIO.contains(&ext) {
        Some(MediaKind::Audio)
    } else if IMAGE.contains(&ext) {
        Some(MediaKind::Image)
    } else {
        None
    }
}

fn build_item(
    path: PathBuf,
    bin: Option<BinId>,
    media: Option<&dyn MediaService>,
) -> Option<MediaItem> {
    let mut kind = kind_of(&path)?;
    let info = media.and_then(|m| m.probe(&path).ok());
    let fingerprint = media.and_then(|m| m.fingerprint(&path).ok());
    if let Some(i) = &info
        && kind == MediaKind::Video
        && i.video.is_none()
        && !i.audio.is_empty()
    {
        kind = MediaKind::Audio;
    }
    Some(MediaItem {
        id: MediaId::new(),
        path,
        kind,
        fingerprint,
        info,
        bin,
        tags: Vec::new(),
        proxy: None,
        generated: None,
    })
}
