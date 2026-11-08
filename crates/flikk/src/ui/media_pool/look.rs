//! How an item looks: thumbnails and waveforms, badges, rings, progress,
//! and the format line.

use crate::ui::format::{clock_rounded, fps_two_places};
use crate::ui::kit::theme as t;
use crate::ui::kit::{Icon, IconName, Spinner, h_flex};
use flikk_core::Time;
use flikk_core::model::{MediaItem, MediaKind};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::sync::Arc;

/// What a tile or row shows of an item, read once per frame.
pub(super) struct Look {
    pub(super) thumb: Option<Arc<Image>>,
    pub(super) name: SharedString,
    pub(super) meta: String,
    pub(super) listenable: bool,
    /// How far the item plays, while it is the source.
    pub(super) listening: Option<f32>,
    pub(super) progress: Option<Progress>,
    pub(super) selected: bool,
    pub(super) offline: bool,
}

/// The 16:9 picture of a tile: the frame, or the kind's mark, the offline
/// veil and the progress along the bottom.
pub(super) fn thumb_box(look: &Look, item: &MediaItem, peaks: Option<&[f32]>) -> Div {
    let bar = look
        .listening
        .map(|f| (f, t::T1))
        .or(look.progress.and_then(Progress::bar));
    div()
        .relative()
        .w_full()
        .aspect_ratio(16. / 9.)
        .rounded(t::RADIUS_SM)
        .overflow_hidden()
        .bg(thumb_bg(item))
        .map(|d| match (&look.thumb, item.kind) {
            (Some(image), _) => {
                d.child(img(image.clone()).size_full().object_fit(ObjectFit::Cover))
            }
            (None, MediaKind::Audio) => match peaks {
                Some(peaks) => d.child(waveform(peaks)),
                None => d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(Icon::new(IconName::AudioLines).size(px(22.)).color(t::A)),
                ),
            },
            (None, MediaKind::Image) => d.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(IconName::Image).size(px(16.)).color(t::T3)),
            ),
            (None, MediaKind::Video) => d,
        })
        .when(look.offline, |d| {
            d.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(t::alpha(t::G, 0.7))
                    .child(Icon::new(IconName::Unlink).size(px(14.)).color(t::WARN)),
            )
        })
        .children(bar.map(|(f, color)| {
            div()
                .absolute()
                .left_0()
                .bottom_0()
                .h(px(2.))
                .w(relative(f.clamp(0., 1.)))
                .bg(color)
        }))
}

/// A sound's peaks across a tile, mirrored about the middle.
fn waveform(peaks: &[f32]) -> Div {
    h_flex()
        .absolute()
        .left(px(6.))
        .right(px(6.))
        .top(px(16.))
        .bottom(px(16.))
        .items_center()
        .gap(px(1.))
        .children(peaks.iter().map(|p| {
            div()
                .flex_1()
                .min_w(px(1.))
                .h(relative(p.clamp(0.04, 1.)))
                .rounded(px(1.))
                .bg(t::alpha(t::A, 0.8))
        }))
}

/// The ground of a thumbnail before (or without) its frame.
pub(super) fn thumb_bg(item: &MediaItem) -> Rgba {
    match item.kind {
        // Opaque: the selection ring is a shadow, which shows through.
        MediaKind::Audio => over(t::A, 0.1, t::P),
        _ => t::R1,
    }
}

/// `c` at `alpha` laid over `ground`, as one opaque colour.
fn over(c: Rgba, alpha: f32, ground: Rgba) -> Rgba {
    let mix = |a: f32, b: f32| b + (a - b) * alpha;
    Rgba {
        r: mix(c.r, ground.r),
        g: mix(c.g, ground.g),
        b: mix(c.b, ground.b),
        a: 1.,
    }
}

/// A small label in a thumbnail's top-left corner.
pub(super) fn thumb_badge() -> Div {
    h_flex()
        .absolute()
        .top(px(3.))
        .left(px(3.))
        .gap(px(3.))
        .h(px(14.))
        .px(px(4.))
        .rounded(px(3.))
        .bg(t::alpha(t::G, 0.67))
        .text_size(px(9.))
        .line_height(px(14.))
        .text_color(t::T1)
}

/// The badge a tile wears in its corner: what is happening to it, else
/// who read it or HDR.
pub(super) fn corner_badge(item: &MediaItem, progress: Option<Progress>) -> Option<Div> {
    match progress {
        Some(Progress::Failed) => {
            return Some(
                thumb_badge()
                    .text_color(t::ERR)
                    .child(Icon::new(IconName::CircleAlert).size(px(9.)))
                    .child("Failed"),
            );
        }
        Some(Progress::Analyzing(_)) => {
            return Some(
                thumb_badge()
                    .child(Spinner::new().size(px(8.)))
                    .child("Analyzing"),
            );
        }
        Some(Progress::Proxy(f)) => {
            return Some(
                thumb_badge()
                    .font_family(t::mono())
                    .text_color(t::T2)
                    .child(format!("Proxy {:.0} %", f * 100.)),
            );
        }
        Some(Progress::Queued) | None => {}
    }
    let reading = item.generated.as_deref().and_then(|g| g.reading.as_ref());
    if let Some(reading) = reading {
        return Some(
            thumb_badge()
                .child(Icon::new(IconName::Mic).size(px(9.)))
                .child(reading.voice_name.clone()),
        );
    }
    flikk_core::hdr::hdr_of(item)?;
    Some(thumb_badge().font_weight(FontWeight::SEMIBOLD).child("HDR"))
}

/// The duration over a thumbnail's bottom-right corner.
pub(super) fn duration_chip(duration: Time) -> impl IntoElement {
    div()
        .absolute()
        .right(px(3.))
        .bottom(px(3.))
        .px(px(3.))
        .rounded(px(2.))
        .bg(t::alpha(t::G, 0.67))
        .font_family(t::mono())
        .font_weight(FontWeight::MEDIUM)
        .text_size(px(9.))
        .text_color(t::T1)
        .child(clock_rounded(duration))
}

/// Selection: a 1.5 px accent ring around a thumbnail.
pub(super) fn ring() -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: t::AC.into(),
        offset: point(px(0.), px(0.)),
        blur_radius: px(0.),
        spread_radius: px(1.5),
        inset: false,
    }]
}

/// A 1 px ring inside a selected row.
pub(super) fn inset_ring(color: Rgba) -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: color.into(),
        offset: point(px(0.), px(0.)),
        blur_radius: px(0.),
        spread_radius: px(1.),
        inset: true,
    }]
}

/// "4K · 25p · used ×3", "WAV · 48 kHz", "Voiceover · 112 words".
pub(super) fn meta(item: &MediaItem, used: usize) -> String {
    let mut parts: Vec<String> = Vec::new();
    let ext = item
        .path
        .extension()
        .map(|e| e.to_string_lossy().to_uppercase());
    let script = item
        .generated
        .as_deref()
        .filter(|g| g.reading.is_some())
        .map(|g| g.prompt.split_whitespace().count());
    if let Some(words) = script {
        parts.push("Voiceover".into());
        parts.push(format!("{words} words"));
    } else if let Some(v) = item.info.as_ref().and_then(|i| i.video.as_ref()) {
        if item.kind == MediaKind::Image {
            parts.extend(ext);
            parts.push(format!("{}×{}", v.width, v.height));
        } else {
            parts.push(resolution(v.width, v.height));
            parts.push(format!("{}p", fps_two_places(v.rate.fps())));
        }
    } else {
        parts.extend(ext);
        if let Some(a) = item.info.as_ref().and_then(|i| i.audio.first()) {
            parts.push(format!("{} kHz", a.sample_rate / 1000));
        }
    }
    if used > 0 {
        parts.push(format!("used ×{used}"));
    }
    parts.join(" · ")
}

/// The list's format column: picture size and rate, or the first sound.
pub(super) fn format_of(item: &MediaItem) -> String {
    let Some(info) = &item.info else {
        return String::new();
    };
    if let Some(v) = &info.video {
        if item.kind == MediaKind::Image {
            return format!("{}×{}", v.width, v.height);
        }
        return format!(
            "{}×{} · {} fps",
            v.width,
            v.height,
            fps_two_places(v.rate.fps())
        );
    }
    info.audio
        .first()
        .map(|a| {
            format!(
                "A{} · {} ch · {} kHz",
                a.index,
                a.channels,
                a.sample_rate / 1000
            )
        })
        .unwrap_or_default()
}

/// "4K", "6K", or the height for HD and smaller.
fn resolution(width: u32, height: u32) -> String {
    match width {
        w if w >= 7680 => "8K".into(),
        w if w >= 6144 => "6K".into(),
        w if w >= 5120 => "5K".into(),
        w if w >= 3840 => "4K".into(),
        _ => height.to_string(),
    }
}

/// "MP4", "WAV": the file's extension, else the first name the
/// container goes by ("mov,mp4,m4a,…" is one demuxer).
pub(super) fn file_type(item: &MediaItem) -> Option<String> {
    let ext = item
        .path
        .extension()
        .map(|e| e.to_string_lossy().to_uppercase());
    let container = item.info.as_ref().and_then(|i| {
        let first = i.container.split(',').next()?.trim();
        (!first.is_empty()).then(|| first.to_uppercase())
    });
    ext.or(container)
}

#[derive(Clone, Copy)]
pub(super) enum Progress {
    Proxy(f32),
    Queued,
    Analyzing(f32),
    Failed,
}

impl Progress {
    /// The bar along the thumbnail's bottom: how far, in what colour.
    pub(super) fn bar(self) -> Option<(f32, Rgba)> {
        match self {
            Progress::Proxy(f) => Some((f, t::T2)),
            Progress::Analyzing(f) => Some((f, t::AC)),
            Progress::Queued | Progress::Failed => None,
        }
    }
}
