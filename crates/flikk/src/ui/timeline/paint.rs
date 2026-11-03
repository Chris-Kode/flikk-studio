//! Everything right of the track headers is one canvas: ruler, lanes, clips,
//! waveforms, markers, the change-set overlay, silence bands, the render bar
//! and the playhead.
//! `Scene` is a snapshot taken in `render`, so painting never touches state.

use super::layout::{self, CLIP_INSET, RULER_HEIGHT, Row, Viewport};
use super::overlay::{Mark, OverlayItem};
use super::waveform;
use crate::ui::kit::{IconName, theme as t};
use flikk_core::command::Edge;
use flikk_core::model::{Marker, MarkerKind, TrackKind};
use flikk_core::{ChangeItemId, Rate, Time, TimeRange, TrackId};
use gpui_kit::*;
use std::sync::Arc;

/// Clip time → source played since the clip's start.
pub type Warp = Arc<dyn Fn(Time) -> Time>;

pub struct ClipScene {
    pub range: TimeRange,
    pub label: SharedString,
    /// Clip color label; the track's tint otherwise.
    pub color: Option<Rgba>,
    pub enabled: bool,
    /// Its media file is missing.
    pub offline: bool,
    /// Drawn before the name: disabled, generator, title.
    pub icon: Option<IconName>,
    /// Has a video effect stack: an `fx` badge.
    pub effects: bool,
    /// Retimed: a speed badge such as `150%`.
    pub speed: Option<SharedString>,
    pub selected: bool,
    /// Being dragged: drawn faded at its old place.
    pub lifted: bool,
    /// The trim edge under the pointer.
    pub hover_edge: Option<Edge>,
    pub peaks: Option<Arc<Vec<f32>>>,
    /// Retimed: clip time → source played since its start, which `peaks`
    /// span.
    pub warp: Option<Warp>,
    /// Filmstrip tiles: x offset from the clip start, width, image.
    pub tiles: Vec<(f32, f32, Arc<RenderImage>)>,
    pub fade_in: Time,
    pub fade_out: Time,
}

pub struct TrackScene {
    pub row: Row,
    pub clips: Vec<ClipScene>,
    /// Clip opacity: lower on muted, hidden and locked tracks.
    pub fade: f32,
    pub locked: bool,
    /// A picture track switched off.
    pub hidden: bool,
}

/// Where a dragged clip would land.
pub struct Ghost {
    pub track: TrackId,
    pub range: TimeRange,
    /// Slip and trim offsets, drawn inside the outline.
    pub label: Option<SharedString>,
    /// The edge a trim or roll moves, marked with an accent bar.
    pub edge: Option<Time>,
}

/// A transition block straddling its cut or clip edge.
pub struct TransitionScene {
    pub track: TrackId,
    pub range: TimeRange,
    pub label: SharedString,
    pub selected: bool,
}

/// What an FX or media drag would hit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DropHint {
    /// A whole lane (a sound not yet in the pool).
    Row(TrackId),
    /// A clip (effects).
    Clip(TrackId, TimeRange),
    /// A span that would be filled (a transition, a generator).
    Span(TrackId, TimeRange),
    /// Media landing at `range`, with its sound on `linked`.
    Insert {
        track: TrackId,
        range: TimeRange,
        linked: Option<TrackId>,
    },
}

impl DropHint {
    /// The lanes it tints.
    fn tints(&self, track: TrackId) -> bool {
        match *self {
            DropHint::Row(t) => t == track,
            DropHint::Insert {
                track: t, linked, ..
            } => t == track || linked == Some(track),
            DropHint::Clip(..) | DropHint::Span(..) => false,
        }
    }
}

/// Rubber band in timeline time and scrolled lane y.
#[derive(Clone, Copy, Debug)]
pub struct Marquee {
    pub from: (Time, f32),
    pub to: (Time, f32),
}

pub struct Scene {
    pub viewport: Viewport,
    pub rate: Rate,
    pub tracks: Vec<TrackScene>,
    pub markers: Vec<Marker>,
    pub in_out: Option<TimeRange>,
    pub playhead: Time,
    /// The ruler is being dragged: the playhead shows its timecode.
    pub scrubbing: bool,
    pub overlay: Vec<OverlayItem>,
    pub focused: Option<ChangeItemId>,
    pub hovered: Option<ChangeItemId>,
    pub silences: Vec<TimeRange>,
    pub transitions: Vec<TransitionScene>,
    pub ghosts: Vec<Ghost>,
    pub snap: Option<Time>,
    /// Blade tool hover: the cut position and the track it would cut.
    pub blade: Option<(Time, Option<TrackId>)>,
    pub drop: Option<DropHint>,
    pub marquee: Option<Marquee>,
    /// Which compositor ranges the background render has on disk.
    pub render: Option<Arc<crate::render_cache::Status>>,
}

impl Scene {
    fn row(&self, track: TrackId) -> Option<&Row> {
        self.tracks
            .iter()
            .map(|t| &t.row)
            .find(|r| r.track == track)
    }
}

/// The tint clips on a `kind` track take.
fn tint(kind: TrackKind) -> Rgba {
    match kind {
        TrackKind::Video | TrackKind::Overlay => t::V,
        TrackKind::Audio => t::A,
        TrackKind::Caption => t::C,
        TrackKind::Title => t::TI,
    }
}

/// `c` with its alpha scaled by `k`.
fn fade(c: Rgba, k: f32) -> Rgba {
    t::alpha(c, c.a * k)
}

pub fn paint(scene: Scene, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    let vp = scene.viewport;
    let left = bounds.origin.x;
    let width = bounds.size.width;
    let lanes = Bounds::new(
        point(left, bounds.origin.y + px(RULER_HEIGHT)),
        size(width, (bounds.size.height - px(RULER_HEIGHT)).max(px(0.))),
    );
    let x_of = |t: Time| left + px(vp.x(t));
    let row_top = |row: &Row| lanes.origin.y + px(row.y - vp.scroll_y);
    // A clip's box in its row.
    let clip_box = |row: &Row, range: TimeRange| {
        let (a, b) = (x_of(range.start), x_of(range.end));
        Bounds::new(
            point(a, row_top(row) + px(CLIP_INSET)),
            size(b - a, px(row.height - CLIP_INSET * 2.)),
        )
    };

    window.paint_quad(fill(bounds, t::LANE));

    window.with_content_mask(Some(ContentMask { bounds: lanes }), |window| {
        for track in &scene.tracks {
            let rect = Bounds::new(
                point(left, row_top(&track.row)),
                size(width, px(track.row.height)),
            );
            if track.locked {
                window.paint_quad(fill(rect, pattern_slash(t::alpha(t::T1, 0.04), 6., 6.)));
            }
            if track.hidden {
                window.paint_quad(fill(rect, t::alpha(t::G, 0.4)));
            }
            if scene.drop.is_some_and(|d| d.tints(track.row.track)) {
                window.paint_quad(fill(rect, t::alpha(t::AC, 0.04)));
            }
            window.paint_quad(fill(
                Bounds::new(point(left, rect.bottom() - px(1.)), size(width, px(1.))),
                t::LN,
            ));
        }

        // In/out: dim what is outside.
        if let Some(r) = scene.in_out.filter(|r| !r.is_empty()) {
            let shade = t::alpha(t::G, 0.45);
            let (a, b) = (x_of(r.start), x_of(r.end));
            if a > left {
                window.paint_quad(fill(
                    Bounds::new(lanes.origin, size(a - left, lanes.size.height)),
                    shade,
                ));
            }
            if b < left + width {
                window.paint_quad(fill(
                    Bounds::new(
                        point(b, lanes.origin.y),
                        size(left + width - b, lanes.size.height),
                    ),
                    shade,
                ));
            }
        }

        // Protected ranges: a faint band behind the clips.
        for m in scene
            .markers
            .iter()
            .filter(|m| m.kind == MarkerKind::Protected)
        {
            let (a, b) = (x_of(m.range.start), x_of(m.range.end));
            window.paint_quad(fill(
                Bounds::new(point(a, lanes.origin.y), size(b - a, lanes.size.height)),
                t::alpha(t::WARN, 0.06),
            ));
        }

        for track in &scene.tracks {
            for clip in &track.clips {
                let rect = clip_box(&track.row, clip.range);
                paint_clip(clip, track, rect, window, cx);
            }
        }

        for tr in &scene.transitions {
            if let Some(row) = scene.row(tr.track) {
                paint_transition(tr, row, row_top(row), &x_of, window, cx);
            }
        }

        if let Some(hint) = scene.drop {
            paint_drop(hint, &scene, &clip_box, window, cx);
        }

        // Pending change sets.
        for item in &scene.overlay {
            let Some(row) = item.track.and_then(|t| scene.row(t)) else {
                continue;
            };
            let rect = overlay_rect(item, row, &vp, lanes, left);
            let emphasis = scene.focused == Some(item.item) || scene.hovered == Some(item.item);
            paint_overlay_item(item, rect, emphasis, window);
        }

        // Silence preview.
        for r in &scene.silences {
            let (a, b) = (x_of(r.start), x_of(r.end));
            let band = Bounds::new(
                point(a, lanes.origin.y),
                size((b - a).max(px(1.)), lanes.size.height),
            );
            window.paint_quad(fill(band, t::alpha(t::ERR, 0.22)));
            for x in [a, band.right() - px(1.)] {
                window.paint_quad(fill(
                    Bounds::new(point(x, band.origin.y), size(px(1.), band.size.height)),
                    t::alpha(t::ERR, 0.6),
                ));
            }
        }

        for ghost in &scene.ghosts {
            if let Some(row) = scene.row(ghost.track) {
                paint_ghost(ghost, clip_box(row, ghost.range), &x_of, window, cx);
            }
        }

        if let Some(m) = scene.marquee {
            let (y0, y1) = (m.from.1.min(m.to.1), m.from.1.max(m.to.1));
            let (a, b) = (x_of(m.from.0.min(m.to.0)), x_of(m.from.0.max(m.to.0)));
            let top = lanes.origin.y + px(y0 - vp.scroll_y);
            window.paint_quad(quad(
                Bounds::new(point(a, top), size(b - a, px(y1 - y0))),
                px(0.),
                t::alpha(t::MK, 0.12),
                px(1.),
                t::alpha(t::MK, 0.8),
                BorderStyle::Solid,
            ));
        }

        if let Some((at, track)) = scene.blade {
            let x = x_of(at);
            let (top, h) = match track.and_then(|t| scene.row(t)) {
                Some(row) => (row_top(row), px(row.height)),
                None => (lanes.origin.y, lanes.size.height),
            };
            window.paint_quad(fill(Bounds::new(point(x, top), size(px(1.), h)), t::PH));
            paint_icon(
                IconName::Scissors,
                point(x + px(5.), top + px(4.)),
                px(12.),
                t::T1,
                window,
                cx,
            );
        }
    });

    if let Some(at) = scene.snap {
        window.paint_quad(fill(
            Bounds::new(
                point(x_of(at), bounds.origin.y),
                size(px(1.), bounds.size.height),
            ),
            t::alpha(t::WARN, 0.9),
        ));
    }

    paint_ruler(&scene, bounds, window, cx);
    paint_playhead(&scene, bounds, window, cx);
}

/// Lane rectangle of an overlay item. Added clips that land on committed
/// clips are drawn in the lower part of the lane so both stay visible.
pub fn overlay_rect(
    item: &OverlayItem,
    row: &Row,
    vp: &Viewport,
    lanes: Bounds<Pixels>,
    left: Pixels,
) -> Bounds<Pixels> {
    let top = lanes.origin.y + px(row.y - vp.scroll_y);
    let (a, b) = (
        left + px(vp.x(item.range.start)),
        left + px(vp.x(item.range.end)),
    );
    let w = (b - a).max(px(3.));
    if item.mark == Mark::Added && item.overlaps {
        let h = (row.height * 0.45).max(10.);
        Bounds::new(point(a, top + px(row.height - h - 2.)), size(w, px(h)))
    } else {
        Bounds::new(point(a, top + px(2.)), size(w, px(row.height - 4.)))
    }
}

fn paint_clip(
    clip: &ClipScene,
    track: &TrackScene,
    rect: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let kind = track.row.kind;
    let k = if clip.lifted { 0.35 } else { 1. } * track.fade;
    let hue = clip.color.unwrap_or_else(|| tint(kind));
    let (top, bottom, border, text) = if clip.offline {
        (t::G, t::G, t::alpha(t::WARN, 0.6), t::WARN)
    } else if !clip.enabled {
        let wash = t::alpha(t::T3, 0.12);
        (wash, wash, t::alpha(t::T3, 0.4), t::T3)
    } else {
        match kind {
            TrackKind::Video | TrackKind::Overlay => (
                t::alpha(hue, 0.25),
                t::alpha(hue, 0.15),
                t::alpha(hue, 0.6),
                t::T1,
            ),
            TrackKind::Audio => (
                t::alpha(hue, 0.15),
                t::alpha(hue, 0.15),
                t::alpha(hue, 0.5),
                t::T1,
            ),
            TrackKind::Caption => (
                t::alpha(hue, 0.2),
                t::alpha(hue, 0.2),
                t::alpha(hue, 0.5),
                t::T1,
            ),
            TrackKind::Title => (
                t::alpha(hue, 0.12),
                t::alpha(hue, 0.12),
                t::alpha(hue, 0.53),
                t::T1,
            ),
        }
    };
    let text = fade(text, k);
    let (border, border_w) = if clip.selected {
        window.paint_quad(quad(
            rect.dilate(px(1.)),
            px(5.),
            transparent_black(),
            px(1.),
            t::alpha(t::AC, 0.33 * k),
            BorderStyle::Solid,
        ));
        (fade(t::AC, k), px(2.))
    } else if clip.hover_edge.is_some() {
        (t::alpha(border, (border.a + 0.3).min(1.) * k), px(1.))
    } else {
        (fade(border, k), px(1.))
    };
    window.paint_quad(quad(
        rect,
        t::RADIUS_SM,
        linear_gradient(
            180.,
            linear_color_stop(fade(top, k), 0.),
            linear_color_stop(fade(bottom, k), 1.),
        ),
        border_w,
        border,
        BorderStyle::Solid,
    ));
    if clip.offline {
        window.paint_quad(
            fill(rect, pattern_slash(t::alpha(t::WARN, 0.1 * k), 5., 5.))
                .corner_radii(t::RADIUS_SM),
        );
    }

    let inner = rect.inset(px(1.));
    window.with_content_mask(Some(ContentMask { bounds: inner }), |window| {
        if !clip.tiles.is_empty() {
            let (offset, h) = layout::filmstrip(f32::from(rect.size.height));
            let strip_top = rect.origin.y + px(offset);
            for (x, w, image) in &clip.tiles {
                let tile = Bounds::new(
                    point(rect.origin.x + px(*x), strip_top),
                    size(px(*w), px(h)),
                );
                let _ = window.paint_image(tile, tile, Corners::default(), image.clone(), 0, false);
                window.paint_quad(fill(
                    Bounds::new(tile.origin, size(px(1.), tile.size.height)),
                    t::G,
                ));
            }
            let strip = Bounds::new(
                point(inner.origin.x, strip_top),
                size(inner.size.width, px(h)),
            );
            let veil = if clip.enabled { 1. - 0.85 * k } else { 0.78 };
            window.paint_quad(fill(strip, t::alpha(t::G, veil)));
        }
        if let Some(peaks) = &clip.peaks {
            paint_waveform(peaks, clip, rect, fade(t::alpha(t::A, 0.7), k), window);
        }
        paint_fades(clip, rect, k, window);
        if let Some(edge) = clip.hover_edge {
            let x = match edge {
                Edge::Start => rect.origin.x,
                Edge::End => rect.right() - px(6.),
            };
            window.paint_quad(fill(
                Bounds::new(point(x, rect.origin.y), size(px(6.), rect.size.height)),
                t::alpha(t::T1, 0.18),
            ));
        }
        if rect.size.width > px(18.) {
            paint_clip_label(clip, rect, text, window, cx);
        }
    });
}

/// The icon, name and badges along a clip's top edge, and the offline note.
fn paint_clip_label(
    clip: &ClipScene,
    rect: Bounds<Pixels>,
    text: Rgba,
    window: &mut Window,
    cx: &mut App,
) {
    let y = if rect.size.height >= px(24.) {
        rect.origin.y + px(2.)
    } else {
        rect.origin.y + (rect.size.height - px(13.)) / 2.
    };
    let mut x = rect.origin.x + px(6.);
    if let Some(icon) = clip.icon.or(clip.offline.then_some(IconName::Unlink)) {
        paint_icon(icon, point(x, y + px(2.)), px(10.), text, window, cx);
        x += px(14.);
    }
    let line = shape(&clip.label, px(10.), text, false, window);
    let _ = line.paint(point(x, y), px(13.), TextAlign::Left, None, window, cx);
    x += line.width + px(4.);
    for badge in clip
        .speed
        .as_deref()
        .into_iter()
        .chain(clip.effects.then_some("fx"))
    {
        let line = shape(badge, px(9.), text, true, window);
        let w = line.width + px(6.);
        if x + w > rect.right() - px(4.) {
            break;
        }
        window.paint_quad(
            fill(
                Bounds::new(point(x, y), size(w, px(13.))),
                t::alpha(t::G, 0.72),
            )
            .corner_radii(px(2.)),
        );
        let _ = line.paint(
            point(x + px(3.), y + px(1.)),
            px(11.),
            TextAlign::Left,
            None,
            window,
            cx,
        );
        x += w + px(4.);
    }
    if clip.offline && rect.size.height >= px(32.) {
        paint_text(
            "Media offline",
            point(rect.origin.x + px(6.), rect.bottom() - px(15.)),
            px(9.),
            t::alpha(t::WARN, 0.67),
            window,
            cx,
        );
    }
}

fn paint_transition(
    tr: &TransitionScene,
    row: &Row,
    top: Pixels,
    x_of: &impl Fn(Time) -> Pixels,
    window: &mut Window,
    cx: &mut App,
) {
    let inset = if row.height >= 30. { 7. } else { CLIP_INSET };
    let (a, b) = (x_of(tr.range.start), x_of(tr.range.end));
    let rect = Bounds::new(
        point(a, top + px(inset)),
        size((b - a).max(px(6.)), px(row.height - inset * 2.)),
    );
    let (border, border_w) = if tr.selected {
        window.paint_quad(quad(
            rect.dilate(px(1.)),
            px(4.),
            transparent_black(),
            px(1.),
            t::alpha(t::AC, 0.33),
            BorderStyle::Solid,
        ));
        (t::AC, px(2.))
    } else {
        (t::alpha(t::ON_AC, 0.33), px(1.))
    };
    window.paint_quad(quad(
        rect,
        px(3.),
        linear_gradient(
            90.,
            linear_color_stop(t::alpha(t::V, 0.33), 0.),
            linear_color_stop(t::alpha(t::ON_AC, 0.13), 1.),
        ),
        border_w,
        border,
        BorderStyle::Solid,
    ));
    let color = if tr.selected {
        t::ON_AC
    } else {
        t::alpha(t::ON_AC, 0.8)
    };
    let label = shape(&tr.label, px(9.), color, true, window);
    if label.width + px(8.) < rect.size.width {
        let origin = point(
            rect.origin.x + (rect.size.width - label.width) / 2.,
            rect.origin.y + (rect.size.height - px(12.)) / 2.,
        );
        let _ = label.paint(origin, px(12.), TextAlign::Left, None, window, cx);
    }
}

fn paint_ghost(
    ghost: &Ghost,
    rect: Bounds<Pixels>,
    x_of: &impl Fn(Time) -> Pixels,
    window: &mut Window,
    cx: &mut App,
) {
    let rect = rect.dilate(px(1.));
    window.paint_quad(quad(
        rect,
        t::RADIUS_SM,
        t::alpha(t::T1, 0.08),
        px(1.5),
        t::alpha(t::T1, 0.9),
        BorderStyle::Solid,
    ));
    if let Some(at) = ghost.edge {
        window.paint_quad(
            fill(
                Bounds::new(
                    point(x_of(at) - px(2.), rect.origin.y),
                    size(px(4.), rect.size.height),
                ),
                t::AC,
            )
            .corner_radii(px(2.)),
        );
    }
    if let Some(label) = &ghost.label {
        let line = shape(label, px(10.), t::T1, true, window);
        let origin = point(
            rect.origin.x + (rect.size.width - line.width) / 2.,
            rect.origin.y + (rect.size.height - px(14.)) / 2.,
        );
        window.paint_quad(
            fill(
                Bounds::new(
                    origin - point(px(4.), px(1.)),
                    size(line.width + px(8.), px(16.)),
                ),
                t::alpha(t::G, 0.85),
            )
            .corner_radii(px(3.)),
        );
        let _ = line.paint(origin, px(14.), TextAlign::Left, None, window, cx);
    }
}

fn paint_drop(
    hint: DropHint,
    scene: &Scene,
    clip_box: &impl Fn(&Row, TimeRange) -> Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let target = |track: TrackId, range: TimeRange, fill: Rgba, border: Rgba, w: f32| {
        scene.row(track).map(|row| {
            let mut rect = clip_box(row, range);
            rect.size.width = rect.size.width.max(px(4.));
            quad(rect, t::RADIUS_SM, fill, px(w), border, BorderStyle::Dashed)
        })
    };
    match hint {
        DropHint::Row(_) => {}
        DropHint::Clip(track, range) => {
            if let Some(q) = target(track, range, t::alpha(t::T1, 0.06), t::T1, 1.5) {
                window.paint_quad(q);
            }
        }
        DropHint::Span(track, range) => {
            if let Some(q) = target(track, range, t::alpha(t::AC, 0.12), t::AC, 1.5) {
                window.paint_quad(q);
            }
        }
        DropHint::Insert {
            track,
            range,
            linked,
        } => {
            let rows: Vec<&Row> = [Some(track), linked]
                .into_iter()
                .flatten()
                .filter_map(|t| scene.row(t))
                .collect();
            if let Some(q) = target(track, range, t::alpha(t::AC, 0.12), t::AC, 1.) {
                window.paint_quad(q);
            }
            if let Some(q) = linked
                .and_then(|l| target(l, range, t::alpha(t::A, 0.08), t::alpha(t::A, 0.67), 1.))
            {
                window.paint_quad(q);
            }
            let (Some(first), Some(last)) = (
                rows.iter()
                    .map(|r| clip_box(r, range).origin.y)
                    .reduce(Pixels::min),
                rows.iter()
                    .map(|r| clip_box(r, range).bottom())
                    .reduce(Pixels::max),
            ) else {
                return;
            };
            let x = clip_box(rows[0], range).origin.x;
            let line = Bounds::new(
                point(x - px(1.), first - px(5.)),
                size(px(2.), last - first + px(10.)),
            );
            window.paint_quad(fill(line.dilate(px(2.)), t::alpha(t::AC, 0.2)).corner_radii(px(3.)));
            window.paint_quad(fill(line, t::AC).corner_radii(px(1.)));
            let label = shape(
                &range.start.timecode(scene.rate),
                px(10.),
                t::AC,
                true,
                window,
            );
            let origin = point(x - px(1.), last + px(10.));
            window.paint_quad(
                fill(
                    Bounds::new(origin, size(label.width + px(10.), px(16.))),
                    t::alpha(t::AC, 0.12),
                )
                .corner_radii(px(3.)),
            );
            let _ = label.paint(
                origin + point(px(5.), px(1.)),
                px(14.),
                TextAlign::Left,
                None,
                window,
                cx,
            );
        }
    }
}

/// Bars 1.5 px wide every 2.5 px, mirrored about the middle of the band
/// under the clip's name.
fn paint_waveform(
    peaks: &[f32],
    clip: &ClipScene,
    rect: Bounds<Pixels>,
    color: Rgba,
    window: &mut Window,
) {
    let width = f32::from(rect.size.width);
    if width < 2. {
        return;
    }
    let (offset, h) = layout::filmstrip(f32::from(rect.size.height));
    let mid = rect.origin.y + px(offset + h / 2.);
    let amp = h / 2.;
    let len = clip.range.duration();
    let visible = window.content_mask().bounds;
    let x0 = f32::from(visible.origin.x - rect.origin.x).max(0.);
    let x1 = f32::from(visible.right() - rect.origin.x).min(width);
    let step = 2.5;
    let mut x = x0 - x0.rem_euclid(step);
    while x < x1 {
        let from = Time((len.0 as f64 * (x / width) as f64) as i64);
        let to = Time((len.0 as f64 * ((x + step) / width) as f64) as i64);
        let p = match &clip.warp {
            Some(warp) => waveform::peak(peaks, warp(len), warp(from), warp(to)),
            None => waveform::peak(peaks, len, from, to),
        }
        .clamp(0., 1.);
        let bar = (amp * p).max(0.5);
        window.paint_quad(fill(
            Bounds::new(
                point(rect.origin.x + px(x), mid - px(bar)),
                size(px(1.5), px(bar * 2.)),
            ),
            color,
        ));
        x += step;
    }
}

/// Fade ramps: the faded corner darkened under a diagonal line.
fn paint_fades(clip: &ClipScene, rect: Bounds<Pixels>, k: f32, window: &mut Window) {
    let len = clip.range.duration().0 as f32;
    if len <= 0. {
        return;
    }
    let w = rect.size.width;
    let (top, bottom) = (rect.origin.y, rect.bottom());
    for (fade_len, start) in [(clip.fade_in, true), (clip.fade_out, false)] {
        if fade_len.0 <= 0 {
            continue;
        }
        let fw = w * (fade_len.0 as f32 / len).min(1.);
        let (edge, inner) = if start {
            (rect.origin.x, rect.origin.x + fw)
        } else {
            (rect.right(), rect.right() - fw)
        };
        let mut shade = PathBuilder::fill();
        shade.move_to(point(edge, top));
        shade.line_to(point(inner, top));
        shade.line_to(point(edge, bottom));
        shade.close();
        if let Ok(path) = shade.build() {
            window.paint_path(path, t::alpha(t::G, 0.6 * k));
        }
        let mut ramp = PathBuilder::stroke(px(1.));
        ramp.move_to(point(edge, bottom));
        ramp.line_to(point(inner, top));
        if let Ok(path) = ramp.build() {
            window.paint_path(path, t::alpha(t::T1, 0.67 * k));
        }
    }
}

fn paint_overlay_item(
    item: &OverlayItem,
    rect: Bounds<Pixels>,
    emphasis: bool,
    window: &mut Window,
) {
    match item.mark {
        Mark::Added => {
            let rect = rect.inset(px(1.));
            let base = if emphasis { 0.36 } else { 0.24 };
            window.paint_quad(quad(
                rect,
                t::RADIUS_SM,
                t::alpha(t::OK, if item.overlaps { 0.13 } else { base }),
                px(if emphasis { 2. } else { 1.5 }),
                t::OK,
                BorderStyle::Dashed,
            ));
            if item.overlaps {
                window.paint_quad(
                    fill(rect, pattern_slash(t::alpha(t::OK, 0.27), 3., 4.))
                        .corner_radii(t::RADIUS_SM),
                );
            }
        }
        Mark::Removed => {
            let (base, stripe) = if emphasis { (0.2, 0.3) } else { (0.08, 0.17) };
            window.paint_quad(fill(rect, t::alpha(t::ERR, base)).corner_radii(px(3.)));
            window.paint_quad(
                fill(rect, pattern_slash(t::alpha(t::ERR, stripe), 4., 4.)).corner_radii(px(3.)),
            );
            let (w, color, style) = if emphasis {
                (1.5, t::ERR, BorderStyle::Solid)
            } else {
                (1., t::alpha(t::ERR, 0.67), BorderStyle::Dashed)
            };
            window.paint_quad(quad(rect, px(3.), transparent_black(), px(w), color, style));
        }
        Mark::Modified => {
            window.paint_quad(quad(
                rect,
                t::RADIUS_SM,
                t::alpha(t::WARN, if emphasis { 0.14 } else { 0.06 }),
                px(if emphasis { 2. } else { 1.5 }),
                t::WARN,
                BorderStyle::Solid,
            ));
        }
        Mark::Cut => {
            let w = if emphasis { px(2.) } else { px(1.5) };
            window.paint_quad(fill(
                Bounds::new(
                    point(rect.origin.x - w / 2., rect.origin.y),
                    size(w, rect.size.height),
                ),
                t::alpha(t::ERR, if emphasis { 1. } else { 0.8 }),
            ));
        }
    }
    if !item.warnings.is_empty() {
        let o = point(rect.right() - px(11.), rect.origin.y + px(3.));
        let mut tri = PathBuilder::fill();
        tri.move_to(point(o.x + px(4.), o.y));
        tri.line_to(point(o.x + px(8.), o.y + px(7.)));
        tri.line_to(point(o.x, o.y + px(7.)));
        tri.close();
        if let Ok(path) = tri.build() {
            window.paint_path(path, t::WARN);
        }
    }
}

fn paint_ruler(scene: &Scene, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    let vp = scene.viewport;
    let left = bounds.origin.x;
    let width = bounds.size.width;
    let ruler = Bounds::new(bounds.origin, size(width, px(RULER_HEIGHT)));
    window.paint_quad(fill(ruler, t::P));
    window.paint_quad(fill(
        Bounds::new(point(left, ruler.bottom() - px(1.)), size(width, px(1.))),
        t::LN,
    ));
    let x_of = |t: Time| left + px(vp.x(t));

    window.with_content_mask(Some(ContentMask { bounds: ruler }), |window| {
        if let Some(r) = scene.in_out.filter(|r| !r.is_empty()) {
            let (a, b) = (x_of(r.start), x_of(r.end));
            let band = Bounds::new(
                point(a, ruler.origin.y),
                size((b - a).max(px(1.)), ruler.size.height - px(1.)),
            );
            window.paint_quad(fill(band, t::alpha(t::AC, 0.08)));
            for x in [a, band.right() - px(1.)] {
                window.paint_quad(fill(
                    Bounds::new(point(x, band.origin.y), size(px(1.), band.size.height)),
                    t::alpha(t::AC, 0.53),
                ));
            }
        }

        let ticks = layout::ticks(vp.px_per_sec, scene.rate);
        let first = vp.time(0.).0.max(0);
        let last = vp.time(f32::from(width)).0;
        let minor = ticks.minor.0.max(1);
        let mut at = first - first.rem_euclid(minor);
        while at <= last {
            let x = x_of(Time(at));
            let major = at % ticks.major.0.max(1) == 0;
            let (h, color) = if major {
                (8., t::T3)
            } else {
                (4., t::alpha(t::T3, 0.53))
            };
            window.paint_quad(fill(
                Bounds::new(point(x, ruler.bottom() - px(h + 1.)), size(px(1.), px(h))),
                color,
            ));
            if major {
                let label = layout::ruler_label(Time(at), scene.rate, ticks.frames);
                let line = shape(&label, px(10.), t::T3, true, window);
                let _ = line.paint(
                    point(x + px(4.), ruler.origin.y + px(3.)),
                    px(13.),
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }
            at += minor;
        }

        // The render bar, along the top edge.
        for (range, cached) in scene.render.iter().flat_map(|s| &s.runs) {
            let (a, b) = (x_of(range.start), x_of(range.end));
            if b < ruler.origin.x || a > ruler.right() {
                continue;
            }
            let color = if *cached {
                t::alpha(t::OK, 0.8)
            } else {
                t::alpha(t::ERR, 0.7)
            };
            window.paint_quad(fill(
                Bounds::new(point(a, ruler.origin.y), size((b - a).max(px(1.)), px(3.))),
                color,
            ));
        }

        for m in &scene.markers {
            let color = match m.kind {
                MarkerKind::Note => t::MK,
                MarkerKind::Protected => t::WARN,
            };
            paint_marker(m.range, color, &vp, ruler, window);
        }
        for item in scene.overlay.iter().filter(|i| i.track.is_none()) {
            let color = match item.mark {
                Mark::Added => t::OK,
                Mark::Removed | Mark::Cut => t::ERR,
                Mark::Modified => t::WARN,
            };
            paint_marker(item.range, color, &vp, ruler, window);
        }
    });
}

/// A diamond on the ruler, with a line to the end of a range marker.
fn paint_marker(
    range: TimeRange,
    color: Rgba,
    vp: &Viewport,
    ruler: Bounds<Pixels>,
    window: &mut Window,
) {
    let a = ruler.origin.x + px(vp.x(range.start));
    let b = ruler.origin.x + px(vp.x(range.end));
    let y = ruler.origin.y + px(7.);
    if b - a > px(4.) {
        window.paint_quad(fill(
            Bounds::new(point(a, y - px(1.)), size(b - a, px(2.))),
            t::alpha(color, 0.5),
        ));
    }
    let r = px(4.5);
    let mut diamond = PathBuilder::fill();
    diamond.move_to(point(a, y - r));
    diamond.line_to(point(a + r, y));
    diamond.line_to(point(a, y + r));
    diamond.line_to(point(a - r, y));
    diamond.close();
    if let Ok(path) = diamond.build() {
        window.paint_path(path, color);
    }
}

fn paint_playhead(scene: &Scene, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    let left = bounds.origin.x;
    let x = left + px(scene.viewport.x(scene.playhead));
    if x < left - px(6.) || x > left + bounds.size.width + px(6.) {
        return;
    }
    let top = bounds.origin.y;
    window.paint_quad(fill(
        Bounds::new(point(x, top), size(px(1.), bounds.size.height)),
        t::PH,
    ));
    let (l, r) = (x - px(5.), x + px(6.));
    let mut head = PathBuilder::fill();
    head.move_to(point(l, top));
    head.line_to(point(r, top));
    head.line_to(point(r, top + px(7.2)));
    head.line_to(point(x + px(0.5), top + px(12.)));
    head.line_to(point(l, top + px(7.2)));
    head.close();
    if let Ok(path) = head.build() {
        window.paint_path(path, t::PH);
    }
    if scene.scrubbing {
        let line = shape(
            &scene.playhead.timecode(scene.rate),
            px(10.),
            t::ON_AC,
            true,
            window,
        );
        let chip = Bounds::new(
            point(x + px(8.), top + px(3.)),
            size(line.width + px(10.), px(15.)),
        );
        window.paint_quad(fill(chip, t::PH).corner_radii(px(3.)));
        let _ = line.paint(
            chip.origin + point(px(5.), px(1.)),
            px(13.),
            TextAlign::Left,
            None,
            window,
            cx,
        );
    }
}

fn paint_icon(
    name: IconName,
    origin: Point<Pixels>,
    side: Pixels,
    color: Rgba,
    window: &mut Window,
    cx: &App,
) {
    let _ = window.paint_svg(
        Bounds::new(origin, size(side, side)),
        name.path(),
        None,
        TransformationMatrix::unit(),
        color.into(),
        cx,
    );
}

fn shape(
    text: &str,
    font_size: Pixels,
    color: Rgba,
    mono: bool,
    window: &mut Window,
) -> ShapedLine {
    let run = TextRun {
        len: text.len(),
        font: if mono {
            font(t::mono())
        } else {
            window.text_style().font()
        },
        color: color.into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window.text_system().shape_line(
        SharedString::from(text.to_string()),
        font_size,
        &[run],
        None,
    )
}

fn paint_text(
    text: &str,
    origin: Point<Pixels>,
    font_size: Pixels,
    color: Rgba,
    window: &mut Window,
    cx: &mut App,
) {
    let line = shape(text, font_size, color, false, window);
    let _ = line.paint(origin, font_size * 1.3, TextAlign::Left, None, window, cx);
}
