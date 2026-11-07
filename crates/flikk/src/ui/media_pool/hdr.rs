//! Proxies of the pool's videos made again when missing: cleared in
//! Settings → Storage, removed unused (`flikk_core::cache_sweep`), or, for
//! HDR items, which are made already mapped to SDR, when an item's tone
//! map changes (`flikk_core::hdr`). The media engine finds a proxy where
//! it is made, so `MediaItem::proxy` is left alone.

use super::MediaPoolPanel;
use flikk_core::MediaId;
use flikk_core::hdr;
use flikk_core::model::{MediaItem, MediaKind};
use gpui_kit::*;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How long a failed proxy waits before it is made again.
const RETRY: Duration = Duration::from_secs(30);

/// A proxy build started this session.
pub(super) enum Tried {
    /// Running, or made.
    Started,
    /// Failed then; started again after [`RETRY`].
    Failed(Instant),
}

/// What an item's proxy is mapped with; changes with its tone map.
pub(super) fn tone_key(item: &MediaItem) -> Option<String> {
    hdr::Mapping::of(item).map(|m| m.key())
}

impl MediaPoolPanel {
    /// Start the proxy of each online video that has none on disk (for an
    /// HDR one, none for its tone map); each file is made once a session,
    /// or again [`RETRY`] after it failed.
    pub(super) fn refresh_proxies(&mut self, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let Some(media) = state.services.media.clone() else {
            return;
        };
        let busy = self.proxies.lock().unwrap();
        let wanted: Vec<(MediaItem, PathBuf)> = state
            .bus
            .project()
            .media
            .iter()
            .filter(|m| m.kind == MediaKind::Video && !busy.contains_key(&m.id))
            .filter_map(|m| Some((m, flikk_media::proxy_path_for(m)?)))
            .filter(|(_, path)| match self.hdr_proxies.get(path) {
                None => true,
                Some(Tried::Started) => false,
                Some(Tried::Failed(at)) => at.elapsed() >= RETRY,
            })
            .map(|(item, path)| (item.clone(), path))
            .collect();
        drop(busy);
        for (item, path) in wanted {
            // An SDR item's proxy may be where an older version made it.
            let elsewhere =
                hdr::hdr_of(&item).is_none() && item.proxy.as_ref().is_some_and(|p| p.is_file());
            if elsewhere || path.is_file() {
                // Made: not looked for again this session.
                self.hdr_proxies.insert(path, Tried::Started);
                continue;
            }
            if !item.path.is_file() {
                // Offline: looked for again after a while, not every poll.
                self.hdr_proxies.insert(path, Tried::Failed(Instant::now()));
                continue;
            }
            // Marked only now a build starts, so one skipped while another
            // ran (say, during the import's) is still made.
            self.hdr_proxies.insert(path.clone(), Tried::Started);
            let (id, key) = (item.id, tone_key(&item));
            let (media, progress) = (media.clone(), self.proxies.clone());
            progress.lock().unwrap().insert(id, 0.0);
            cx.spawn(async move |this, cx| {
                let report = progress.clone();
                let made = cx
                    .background_executor()
                    .spawn(async move {
                        media
                            .ensure_proxy(&item, &|f| {
                                report.lock().unwrap().insert(id, f);
                            })
                            .is_ok()
                    })
                    .await;
                progress.lock().unwrap().remove(&id);
                this.update(cx, |this, cx| {
                    this.proxy_built(id, Some(path), key, made, cx)
                })
                .ok();
            })
            .detach();
        }
    }

    /// A build of proxy `path`, mapped with `key`, has ended: a failure is
    /// noted for a retry, and a tone map changed meanwhile makes the
    /// proxy for the new one.
    pub(super) fn proxy_built(
        &mut self,
        id: MediaId,
        path: Option<PathBuf>,
        key: Option<String>,
        made: bool,
        cx: &mut Context<Self>,
    ) {
        if !made && let Some(path) = path {
            self.hdr_proxies.insert(path, Tried::Failed(Instant::now()));
        }
        let project = self.state.read(cx).bus.project();
        if project.media(id).is_some_and(|m| tone_key(m) != key) {
            self.refresh_proxies(cx);
        }
        cx.notify();
    }
}
