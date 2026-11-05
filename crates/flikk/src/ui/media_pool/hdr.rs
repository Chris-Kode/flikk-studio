//! Proxies of the pool's videos made again when missing: cleared in
//! Settings → Storage, removed unused (`flikk_core::cache_sweep`), or, for
//! HDR items, which are made already mapped to SDR, when an item's tone
//! map changes (`flikk_core::hdr`). The media engine finds a proxy where
//! it is made, so `MediaItem::proxy` is left alone.

use super::MediaPoolPanel;
use flikk_core::hdr;
use flikk_core::model::{MediaItem, MediaKind};
use gpui_kit::*;

impl MediaPoolPanel {
    /// Start the proxy of each online video that has none on disk (for an
    /// HDR one, none for its tone map); each file is tried once a session.
    pub(super) fn refresh_proxies(&mut self, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let Some(media) = state.services.media.clone() else {
            return;
        };
        let wanted: Vec<(MediaItem, std::path::PathBuf)> = state
            .bus
            .project()
            .media
            .iter()
            .filter(|m| m.kind == MediaKind::Video)
            .filter_map(|m| Some((m.clone(), flikk_media::proxy_path_for(m)?)))
            .filter(|(_, path)| !self.hdr_proxies.contains(path))
            .collect();
        for (item, path) in wanted {
            self.hdr_proxies.insert(path.clone());
            // An SDR item's proxy may be where an older version made it.
            let elsewhere =
                hdr::hdr_of(&item).is_none() && item.proxy.as_ref().is_some_and(|p| p.is_file());
            if elsewhere
                || path.is_file()
                || !item.path.is_file()
                || self.proxies.lock().unwrap().contains_key(&item.id)
            {
                continue;
            }
            let (id, media, progress) = (item.id, media.clone(), self.proxies.clone());
            progress.lock().unwrap().insert(id, 0.0);
            cx.spawn(async move |this, cx| {
                let report = progress.clone();
                cx.background_executor()
                    .spawn(async move {
                        let _ = media.ensure_proxy(&item, &|f| {
                            report.lock().unwrap().insert(id, f);
                        });
                    })
                    .await;
                progress.lock().unwrap().remove(&id);
                this.update(cx, |_, cx| cx.notify()).ok();
            })
            .detach();
        }
    }
}
