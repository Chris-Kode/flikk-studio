//! HDR items in the pool: their badge, and their proxies, which are made
//! already mapped to SDR, so one is made again when an item's tone map
//! changes (`koo_core::hdr`). The media engine finds an HDR item's proxy by
//! its tone map, so `MediaItem::proxy` is left alone.

use super::MediaPoolPanel;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;
use koo_core::hdr;
use koo_core::model::{MediaItem, MediaKind};

/// "HDR" over the thumbnail's corner.
pub(super) fn badge(item: &MediaItem, cx: &App) -> Option<Div> {
    hdr::hdr_of(item)?;
    let theme = cx.theme();
    Some(
        div()
            .absolute()
            .top(px(2.))
            .left(px(2.))
            .px(px(3.))
            .rounded_sm()
            .bg(theme.background.opacity(0.75))
            .text_color(theme.foreground)
            .text_size(px(8.))
            .line_height(px(11.))
            .font_weight(FontWeight::SEMIBOLD)
            .child("HDR"),
    )
}

impl MediaPoolPanel {
    /// Start the proxy of each HDR video whose tone map has none yet; each
    /// file is tried once a session.
    pub(super) fn refresh_hdr_proxies(&mut self, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let Some(media) = state.services.media.clone() else {
            return;
        };
        let wanted: Vec<(MediaItem, std::path::PathBuf)> = state
            .bus
            .project()
            .media
            .iter()
            .filter(|m| m.kind == MediaKind::Video && hdr::hdr_of(m).is_some())
            .filter_map(|m| Some((m.clone(), koo_media::proxy_path_for(m)?)))
            .filter(|(_, path)| !self.hdr_proxies.contains(path))
            .collect();
        for (item, path) in wanted {
            self.hdr_proxies.insert(path.clone());
            if path.is_file() || self.proxies.lock().unwrap().contains_key(&item.id) {
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
