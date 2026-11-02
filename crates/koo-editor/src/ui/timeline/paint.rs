//! Everything right of the track headers is one canvas: ruler, lanes, clips,
//! waveforms, markers, the change-set overlay, silence bands, the render bar
//! and the playhead.
//! `Scene` is a snapshot taken in `render`, so painting never touches state.

use super::layout::{self, RULER_HEIGHT, Row, Viewport};
use super::overlay::{Mark, OverlayItem};
use super::waveform;
use gpui_kit::component::Theme;
use gpui_kit::*;
use koo_core::model::{Marker, MarkerKind, TrackKind};
use koo_core::{ChangeItemId, Rate, Time, TimeRange, TrackId};
use std::sync::Arc;

#[derive(Clone, Copy)]
pub struct Colors {
    pub background: Hsla,
    pub lane: Hsla,
    pub border: Hsla,
    pub foreground: Hsla,
    pub muted: Hsla,
    pub video: Hsla,
    pub audio: Hsla,
    pub caption: Hsla,
    pub title: Hsla,
    pub green: Hsla,
    pub red: Hsla,
    pub yellow: Hsla,
    pub marker: Hsla,
    pub protected: Hsla,
    pub selection: Hsla,
    /// Render bar: frames on disk, and frames still to compose.
    pub cached: Hsla,
    pub uncached: Hsla,
}

impl Colors {
    pub fn new(theme: &Theme) -> Self {
        Self {
            background: theme.background,
            lane: theme.secondary,
            border: theme.border,
            foreground: theme.foreground,
            muted: theme.muted_foreground,
            video: theme.blue,
            audio: theme.cyan,
            caption: theme.magenta,
            title: theme.magenta_light,
            green: theme.green,
            red: theme.red,
            yellow: theme.yellow,
            marker: theme.blue_light,
            protected: theme.warning,
            selection: theme.foreground,
            cached: theme.success,
            uncached: theme.danger,
        }
    }

    fn of(&self, kind: TrackKind) -> Hsla {
        match kind {
            TrackKind::Video | TrackKind::Overlay => self.video,
            TrackKind::Audio => self.audio,
            TrackKind::Caption => self.caption,
            TrackKind::Title => self.title,
        }
    }
}

pub struct ClipScene {
    pub range: TimeRange,
    pub label: SharedString,
    /// Clip color label; the track's color otherwise.
    pub color: Option<Hsla>,
    pub enabled: bool,
    pub generator: bool,
    /// Has a video effect stack: an `fx` badge.
    pub effects: bool,
    /// Retimed: a speed badge such as `150%`.
    pub speed: Option<SharedString>,
    pub selected: bool,
    /// Being dragged: drawn faded at its old place.
    pub lifted: bool,
    pub peaks: Option<Arc<Vec<f32>>>,
    /// Filmstrip tiles: x offset from the clip start, width, image.
    pub tiles: Vec<(f32, f32, Arc<RenderImage>)>,
    pub fade_in: Time,
    pub fade_out: Time,
}

pub struct TrackScene {
    pub row: Row,
    pub clips: Vec<ClipScene>,
    pub dimmed: bool,
}

/// Where a dragged clip would land.
pub struct Ghost {
    pub track: TrackId,
    pub range: TimeRange,
    /// Slip offset and the like, drawn inside the outline.
    pub label: Option<SharedString>,
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
    /// A whole lane (media).
    Row(TrackId),
    /// A clip (effects).
    Clip(TrackId, TimeRange),
    /// A span that would be filled (a transition, a generator).
    Span(TrackId, TimeRange),
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
    pub colors: Colors,
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

pub fn paint(scene: Scene, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    let c = scene.colors;
    let vp = scene.viewport;
    let left = bounds.origin.x;
    let width = bounds.size.width;
    let lanes = Bounds::new(
        point(left, bounds.origin.y + px(RULER_HEIGHT)),
        size(width, (bounds.size.height - px(RULER_HEIGHT)).max(px(0.))),
    );
    let x_of = |t: Time| left + px(vp.x(t));
    let row_top = |row: &Row| lanes.origin.y + px(row.y - vp.scroll_y);

    window.paint_quad(fill(bounds, c.background));

    window.with_content_mask(Some(ContentMask { bounds: lanes }), |window| {
        // Lanes.
        for track in &scene.tracks {
            let top = row_top(&track.row);
            let rect = Bounds::new(
                point(left, top + px(1.)),
                size(width, px(track.row.height - 2.)),
            );
            let tint = if scene.drop == Some(DropHint::Row(track.row.track)) {
                c.lane.opacity(0.9)
            } else {
                c.lane.opacity(0.35)
            };
            window.paint_quad(fill(rect, tint));
        }

        // In/out: dim what is outside.
        if let Some(r) = scene.in_out.filter(|r| !r.is_empty()) {
            let shade = c.background.opacity(0.45);
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
                c.protected.opacity(0.06),
            ));
        }

        for track in &scene.tracks {
            let color = c.of(track.row.kind);
            let top = row_top(&track.row);
            for clip in &track.clips {
                paint_clip(
                    clip,
                    track,
                    color,
                    Bounds::new(
                        point(x_of(clip.range.start), top + px(2.)),
                        size(
                            x_of(clip.range.end) - x_of(clip.range.start),
                            px(track.row.height - 4.),
                        ),
                    ),
                    &c,
                    window,
                    cx,
                );
            }
        }

        for t in &scene.transitions {
            if let Some(row) = scene.row(t.track) {
                let top = row_top(row);
                let (a, b) = (x_of(t.range.start), x_of(t.range.end));
                let rect = Bounds::new(
                    point(a, top + px(2.)),
                    size((b - a).max(px(6.)), px(row.height - 4.)),
                );
                paint_transition(t, rect, &c, window, cx);
            }
        }

        if let Some(hint) = scene.drop {
            let (track, range, filled) = match hint {
                DropHint::Row(_) => (None, None, false),
                DropHint::Clip(t, r) => (Some(t), Some(r), false),
                DropHint::Span(t, r) => (Some(t), Some(r), true),
            };
            if let (Some(row), Some(r)) = (track.and_then(|t| scene.row(t)), range) {
                let top = row_top(row);
                let (a, b) = (x_of(r.start), x_of(r.end));
                let rect = Bounds::new(
                    point(a, top + px(1.)),
                    size((b - a).max(px(4.)), px(row.height - 2.)),
                );
                window.paint_quad(quad(
                    rect,
                    px(4.),
                    c.marker.opacity(if filled { 0.35 } else { 0.12 }),
                    px(2.),
                    c.marker,
                    BorderStyle::Solid,
                ));
            }
        }

        // Pending change sets.
        for item in &scene.overlay {
            let Some(row) = item.track.and_then(|t| scene.row(t)) else {
                continue;
            };
            let rect = overlay_rect(item, row, &vp, lanes, left);
            let emphasis = scene.focused == Some(item.item) || scene.hovered == Some(item.item);
            paint_overlay_item(item, rect, emphasis, &c, window);
        }

        // Silence preview.
        for r in &scene.silences {
            let (a, b) = (x_of(r.start), x_of(r.end));
            let band = Bounds::new(
                point(a, lanes.origin.y),
                size((b - a).max(px(1.)), lanes.size.height),
            );
            window.paint_quad(fill(band, c.red.opacity(0.22)));
            window.paint_quad(fill(
                Bounds::new(band.origin, size(px(1.), band.size.height)),
                c.red.opacity(0.6),
            ));
            window.paint_quad(fill(
                Bounds::new(
                    point(b - px(1.), band.origin.y),
                    size(px(1.), band.size.height),
                ),
                c.red.opacity(0.6),
            ));
        }

        for ghost in &scene.ghosts {
            if let Some(row) = scene.row(ghost.track) {
                let top = row_top(row);
                let rect = Bounds::new(
                    point(x_of(ghost.range.start), top + px(2.)),
                    size(
                        x_of(ghost.range.end) - x_of(ghost.range.start),
                        px(row.height - 4.),
                    ),
                );
                window.paint_quad(quad(
                    rect,
                    px(4.),
                    c.foreground.opacity(0.08),
                    px(1.5),
                    c.foreground.opacity(0.9),
                    BorderStyle::Solid,
                ));
                if let Some(label) = &ghost.label {
                    let line = shape(label, px(11.), c.foreground, window);
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
                            c.background.opacity(0.85),
                        )
                        .corner_radii(px(3.)),
                    );
                    let _ = line.paint(origin, px(14.), TextAlign::Left, None, window, cx);
                }
            }
        }

        if let Some(m) = scene.marquee {
            let (y0, y1) = (m.from.1.min(m.to.1), m.from.1.max(m.to.1));
            let (a, b) = (x_of(m.from.0.min(m.to.0)), x_of(m.from.0.max(m.to.0)));
            let top = lanes.origin.y + px(y0 - vp.scroll_y);
            let rect = Bounds::new(point(a, top), size(b - a, px(y1 - y0)));
            window.paint_quad(quad(
                rect,
                px(0.),
                c.marker.opacity(0.12),
                px(1.),
                c.marker.opacity(0.8),
                BorderStyle::Solid,
            ));
        }

        if let Some((t, track)) = scene.blade {
            let x = x_of(t);
            let (top, h) = match track.and_then(|t| scene.row(t)) {
                Some(row) => (row_top(row), px(row.height)),
                None => (lanes.origin.y, lanes.size.height),
            };
            window.paint_quad(fill(Bounds::new(point(x, top), size(px(1.), h)), c.red));
        }
    });

    if let Some(t) = scene.snap {
        let x = x_of(t);
        window.paint_quad(fill(
            Bounds::new(point(x, lanes.origin.y), size(px(1.), lanes.size.height)),
            c.yellow.opacity(0.9),
        ));
    }

    paint_ruler(&scene, bounds, window, cx);

    // Playhead.
    let x = x_of(scene.playhead);
    if x >= left - px(6.) && x <= left + width + px(6.) {
        window.paint_quad(fill(
            Bounds::new(
                point(x, bounds.origin.y + px(6.)),
                size(px(1.), bounds.size.height),
            ),
            c.foreground,
        ));
        let mut head = PathBuilder::fill();
        let y = bounds.origin.y + px(4.);
        head.move_to(point(x - px(5.5), y));
        head.line_to(point(x + px(6.5), y));
        head.line_to(point(x + px(6.5), y + px(8.)));
        head.line_to(point(x + px(0.5), y + px(14.)));
        head.line_to(point(x - px(5.5), y + px(8.)));
        head.close();
        if let Ok(path) = head.build() {
            window.paint_path(path, c.foreground);
        }
    }
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
        Bounds::new(point(a, top + px(row.height - h - 1.)), size(w, px(h)))
    } else {
        Bounds::new(point(a, top + px(2.)), size(w, px(row.height - 4.)))
    }
}

fn paint_clip(
    clip: &ClipScene,
    track: &TrackScene,
    color: Hsla,
    rect: Bounds<Pixels>,
    c: &Colors,
    window: &mut Window,
    cx: &mut App,
) {
    let color = match (clip.enabled, clip.color) {
        (false, _) => c.muted,
        (true, Some(label)) => label,
        (true, None) => color,
    };
    let alpha = if clip.lifted { 0.35 } else { 1. };
    let dim = if track.dimmed { 0.5 } else { 1. } * if clip.enabled { 1. } else { 0.5 };
    let body = color.opacity(0.32 * alpha * dim);
    let (border, border_w) = if clip.selected {
        (c.selection, px(1.5))
    } else {
        (color.opacity(0.75 * alpha * dim), px(1.))
    };
    window.paint_quad(quad(
        rect,
        px(4.),
        body,
        border_w,
        border,
        BorderStyle::Solid,
    ));
    // A brighter strip on top reads as the clip's "spine".
    window.paint_quad(quad(
        Bounds::new(
            rect.origin + point(px(1.), px(1.)),
            size((rect.size.width - px(2.)).max(px(0.)), px(2.)),
        ),
        Corners {
            top_left: px(3.),
            top_right: px(3.),
            bottom_left: px(0.),
            bottom_right: px(0.),
        },
        color.opacity(0.8 * alpha * dim),
        px(0.),
        transparent_black(),
        BorderStyle::Solid,
    ));

    let inner = Bounds::new(
        rect.origin + point(px(1.), px(1.)),
        size(
            (rect.size.width - px(2.)).max(px(0.)),
            (rect.size.height - px(2.)).max(px(0.)),
        ),
    );
    window.with_content_mask(Some(ContentMask { bounds: inner }), |window| {
        let strip_top = inner.origin.y + px(2.);
        for (offset, w, image) in &clip.tiles {
            let tile = Bounds::new(
                point(rect.origin.x + px(*offset), strip_top),
                size(px(*w), inner.size.height - px(2.)),
            );
            let _ = window.paint_image(tile, tile, Corners::default(), image.clone(), 0, false);
        }
        if !clip.tiles.is_empty() {
            window.paint_quad(fill(inner, color.opacity(0.12)));
            if alpha < 1. || dim < 1. {
                window.paint_quad(fill(inner, c.background.opacity(1. - alpha * dim)));
            }
        }
        if let Some(peaks) = &clip.peaks {
            paint_waveform(peaks, clip, rect, color.opacity(0.55 * alpha * dim), window);
        }
        if clip.generator {
            paint_stripes(inner, color.opacity(0.35 * alpha * dim), window);
        }
        paint_fades(clip, rect, c, window);
        if rect.size.width > px(18.) {
            let text_color = c.foreground.opacity(0.92 * alpha * dim);
            let y = if track.row.height > 32. {
                rect.origin.y + px(4.)
            } else {
                rect.origin.y + (rect.size.height - px(14.)) / 2.
            };
            let origin = point(rect.origin.x.max(inner.origin.x) + px(6.), y);
            if !clip.tiles.is_empty() {
                let line = shape(&clip.label, px(11.), text_color, window);
                window.paint_quad(
                    fill(
                        Bounds::new(
                            origin - point(px(4.), px(1.)),
                            size(line.width + px(8.), px(16.)),
                        ),
                        c.background.opacity(0.6),
                    )
                    .corner_radii(px(3.)),
                );
            }
            paint_text(&clip.label, origin, px(11.), text_color, window, cx);
            paint_badges(clip, rect, track.row.height, text_color, c, window, cx);
        }
    });
}

/// Speed and `fx` badges at the clip's top right, when they fit.
fn paint_badges(
    clip: &ClipScene,
    rect: Bounds<Pixels>,
    row_height: f32,
    text: Hsla,
    c: &Colors,
    window: &mut Window,
    cx: &mut App,
) {
    let badges: Vec<&str> = clip
        .speed
        .as_deref()
        .into_iter()
        .chain(clip.effects.then_some("fx"))
        .collect();
    if badges.is_empty() {
        return;
    }
    let lines: Vec<ShapedLine> = badges
        .iter()
        .map(|b| shape(b, px(10.), text, window))
        .collect();
    let total: Pixels = lines.iter().map(|l| l.width + px(10.)).sum();
    if rect.size.width < total + px(28.) {
        return;
    }
    let y = if row_height > 32. {
        rect.origin.y + px(4.)
    } else {
        rect.origin.y + (rect.size.height - px(14.)) / 2.
    };
    let mut x = rect.right() - px(4.);
    for line in lines.iter().rev() {
        let w = line.width + px(6.);
        x -= w;
        window.paint_quad(
            fill(
                Bounds::new(point(x, y), size(w, px(14.))),
                c.background.opacity(0.8),
            )
            .corner_radii(px(3.)),
        );
        let _ = line.paint(
            point(x + px(3.), y + px(1.)),
            px(12.),
            TextAlign::Left,
            None,
            window,
            cx,
        );
        x -= px(4.);
    }
}

fn paint_transition(
    t: &TransitionScene,
    rect: Bounds<Pixels>,
    c: &Colors,
    window: &mut Window,
    cx: &mut App,
) {
    let (border, border_w) = if t.selected {
        (c.selection, px(1.5))
    } else {
        (c.foreground.opacity(0.55), px(1.))
    };
    window.paint_quad(quad(
        rect,
        px(3.),
        c.background.opacity(0.78),
        border_w,
        border,
        BorderStyle::Solid,
    ));
    if t.selected {
        window.paint_quad(fill(rect, c.selection.opacity(0.16)).corner_radii(px(3.)));
    }
    let inset = px(3.);
    let mut line = PathBuilder::stroke(px(1.));
    line.move_to(point(rect.origin.x + inset, rect.bottom() - inset));
    line.line_to(point(rect.right() - inset, rect.origin.y + inset));
    if let Ok(path) = line.build() {
        window.paint_path(
            path,
            c.foreground.opacity(if t.selected { 0.6 } else { 0.3 }),
        );
    }
    let label = shape(&t.label, px(10.), c.foreground.opacity(0.9), window);
    if label.width + px(12.) < rect.size.width {
        let origin = point(
            rect.origin.x + (rect.size.width - label.width) / 2.,
            rect.origin.y + (rect.size.height - px(13.)) / 2.,
        );
        window.paint_quad(
            fill(
                Bounds::new(
                    origin - point(px(3.), px(1.)),
                    size(label.width + px(6.), px(15.)),
                ),
                c.background.opacity(0.85),
            )
            .corner_radii(px(3.)),
        );
        let _ = label.paint(origin, px(13.), TextAlign::Left, None, window, cx);
    }
}

fn paint_waveform(
    peaks: &[f32],
    clip: &ClipScene,
    rect: Bounds<Pixels>,
    color: Hsla,
    window: &mut Window,
) {
    let width = f32::from(rect.size.width);
    if width < 2. {
        return;
    }
    let len = clip.range.duration();
    let mid = rect.origin.y + rect.size.height * 0.66;
    let amp = rect.size.height * 0.3;
    let visible = window.content_mask().bounds;
    let x0 = f32::from(visible.origin.x - rect.origin.x).max(0.);
    let x1 = f32::from(visible.right() - rect.origin.x).min(width);
    if x1 <= x0 {
        return;
    }
    let step = 2.;
    let mut tops = Vec::new();
    let mut x = x0;
    while x < x1 {
        let from = Time((len.0 as f64 * (x / width) as f64) as i64);
        let to = Time((len.0 as f64 * ((x + step) / width) as f64) as i64);
        let p = waveform::peak(peaks, len, from, to).clamp(0., 1.);
        tops.push((rect.origin.x + px(x), p));
        x += step;
    }
    let mut path = PathBuilder::fill();
    path.move_to(point(tops[0].0, mid));
    for (x, p) in &tops {
        path.line_to(point(*x, mid - amp * *p));
    }
    for (x, p) in tops.iter().rev() {
        path.line_to(point(*x, mid + amp * *p * 0.6));
    }
    path.close();
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

fn paint_fades(clip: &ClipScene, rect: Bounds<Pixels>, c: &Colors, window: &mut Window) {
    let len = clip.range.duration().0 as f32;
    if len <= 0. {
        return;
    }
    let w = rect.size.width;
    for (fade, start) in [(clip.fade_in, true), (clip.fade_out, false)] {
        if fade.0 <= 0 {
            continue;
        }
        let fw = w * (fade.0 as f32 / len).min(1.);
        let (top, bottom) = (rect.origin.y + px(1.), rect.bottom() - px(1.));
        let mut path = PathBuilder::fill();
        if start {
            let x = rect.origin.x;
            path.move_to(point(x, top));
            path.line_to(point(x + fw, top));
            path.line_to(point(x, bottom));
        } else {
            let x = rect.right();
            path.move_to(point(x, top));
            path.line_to(point(x - fw, top));
            path.line_to(point(x, bottom));
        }
        path.close();
        if let Ok(path) = path.build() {
            window.paint_path(path, c.background.opacity(0.45));
        }
    }
}

fn paint_overlay_item(
    item: &OverlayItem,
    rect: Bounds<Pixels>,
    emphasis: bool,
    c: &Colors,
    window: &mut Window,
) {
    let border_w = if emphasis { px(2.) } else { px(1.5) };
    match item.mark {
        Mark::Added => {
            let fill_alpha = if emphasis { 0.38 } else { 0.24 };
            window.paint_quad(quad(
                rect,
                px(4.),
                c.green.opacity(fill_alpha),
                border_w,
                c.green,
                BorderStyle::Dashed,
            ));
            if item.overlaps {
                paint_stripes(rect, c.green.opacity(0.45), window);
            }
        }
        Mark::Removed => {
            // A band, not a box: removed spans of neighbouring clips and
            // tracks merge into one gap.
            window.paint_quad(fill(rect, c.red.opacity(if emphasis { 0.6 } else { 0.45 })));
            let edge = if emphasis { 0.95 } else { 0.7 };
            for x in [rect.origin.x, rect.right() - px(1.)] {
                window.paint_quad(fill(
                    Bounds::new(point(x, rect.origin.y), size(px(1.), rect.size.height)),
                    c.red.opacity(edge),
                ));
            }
        }
        Mark::Modified => {
            window.paint_quad(quad(
                rect,
                px(4.),
                c.yellow.opacity(if emphasis { 0.16 } else { 0.06 }),
                border_w,
                c.yellow,
                BorderStyle::Solid,
            ));
        }
        Mark::Cut => {
            let x = rect.origin.x;
            window.paint_quad(fill(
                Bounds::new(
                    point(x - px(0.5), rect.origin.y),
                    size(border_w, rect.size.height),
                ),
                c.red.opacity(if emphasis { 1. } else { 0.8 }),
            ));
        }
    }
    if !item.warnings.is_empty() {
        let d = px(7.);
        let mut tri = PathBuilder::fill();
        let o = point(rect.right() - d - px(3.), rect.origin.y + px(3.));
        tri.move_to(point(o.x + d / 2., o.y));
        tri.line_to(point(o.x + d, o.y + d));
        tri.line_to(point(o.x, o.y + d));
        tri.close();
        if let Ok(path) = tri.build() {
            window.paint_path(path, c.protected);
        }
    }
}

fn paint_stripes(rect: Bounds<Pixels>, color: Hsla, window: &mut Window) {
    window.with_content_mask(Some(ContentMask { bounds: rect }), |window| {
        let h = rect.size.height;
        let gap = px(7.);
        let mut x = rect.origin.x - h;
        let mut path = PathBuilder::stroke(px(1.));
        let mut any = false;
        while x < rect.right() {
            path.move_to(point(x, rect.bottom()));
            path.line_to(point(x + h, rect.origin.y));
            any = true;
            x += gap;
        }
        if any && let Ok(path) = path.build() {
            window.paint_path(path, color);
        }
    });
}

fn paint_ruler(scene: &Scene, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    let c = scene.colors;
    let vp = scene.viewport;
    let left = bounds.origin.x;
    let width = bounds.size.width;
    let ruler = Bounds::new(bounds.origin, size(width, px(RULER_HEIGHT)));
    window.paint_quad(fill(ruler, c.background));
    window.paint_quad(fill(
        Bounds::new(point(left, ruler.bottom() - px(1.)), size(width, px(1.))),
        c.border,
    ));
    let x_of = |t: Time| left + px(vp.x(t));

    window.with_content_mask(Some(ContentMask { bounds: ruler }), |window| {
        // The render bar, along the top edge.
        for (range, cached) in scene.render.iter().flat_map(|s| &s.runs) {
            let (a, b) = (x_of(range.start), x_of(range.end));
            if b < ruler.origin.x || a > ruler.right() {
                continue;
            }
            let color = if *cached { c.cached } else { c.uncached };
            window.paint_quad(fill(
                Bounds::new(point(a, ruler.origin.y), size((b - a).max(px(1.)), px(2.))),
                color.opacity(0.8),
            ));
        }
        if let Some(r) = scene.in_out {
            let (a, b) = (x_of(r.start), x_of(r.end));
            window.paint_quad(fill(
                Bounds::new(
                    point(a, ruler.origin.y + px(RULER_HEIGHT - 6.)),
                    size((b - a).max(px(2.)), px(5.)),
                ),
                c.foreground.opacity(0.35),
            ));
        }

        let ticks = layout::ticks(vp.px_per_sec, scene.rate);
        let first = vp.time(0.).0.max(0);
        let last = vp.time(f32::from(width)).0;
        let minor = ticks.minor.0.max(1);
        let mut t = first - first.rem_euclid(minor);
        while t <= last {
            let x = x_of(Time(t));
            let major = t % ticks.major.0.max(1) == 0;
            let h = if major { 9. } else { 4. };
            window.paint_quad(fill(
                Bounds::new(
                    point(x, ruler.bottom() - px(h) - px(1.)),
                    size(px(1.), px(h)),
                ),
                if major { c.muted } else { c.muted.opacity(0.5) },
            ));
            if major {
                paint_text(
                    &layout::ruler_label(Time(t), scene.rate, ticks.frames),
                    point(x + px(4.), ruler.origin.y + px(3.)),
                    px(10.),
                    c.muted,
                    window,
                    cx,
                );
            }
            t += minor;
        }

        for m in &scene.markers {
            let color = match m.kind {
                MarkerKind::Note => c.marker,
                MarkerKind::Protected => c.protected,
            };
            paint_marker(m.range, color, &vp, ruler, window);
        }
        for item in scene.overlay.iter().filter(|i| i.track.is_none()) {
            let color = match item.mark {
                Mark::Added => c.green,
                Mark::Removed | Mark::Cut => c.red,
                Mark::Modified => c.yellow,
            };
            paint_marker(item.range, color, &vp, ruler, window);
        }
    });
}

fn paint_marker(
    range: TimeRange,
    color: Hsla,
    vp: &Viewport,
    ruler: Bounds<Pixels>,
    window: &mut Window,
) {
    let a = ruler.origin.x + px(vp.x(range.start));
    let b = ruler.origin.x + px(vp.x(range.end));
    let y = ruler.bottom() - px(8.);
    if b - a > px(2.) {
        window.paint_quad(fill(
            Bounds::new(point(a, y + px(2.)), size(b - a, px(3.))),
            color.opacity(0.75),
        ));
    }
    let mut tag = PathBuilder::fill();
    tag.move_to(point(a - px(4.), y - px(4.)));
    tag.line_to(point(a + px(4.), y - px(4.)));
    tag.line_to(point(a + px(4.), y + px(2.)));
    tag.line_to(point(a, y + px(6.)));
    tag.line_to(point(a - px(4.), y + px(2.)));
    tag.close();
    if let Ok(path) = tag.build() {
        window.paint_path(path, color);
    }
}

fn shape(text: &str, font_size: Pixels, color: Hsla, window: &mut Window) -> ShapedLine {
    let run = TextRun {
        len: text.len(),
        font: window.text_style().font(),
        color,
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

pub fn paint_text(
    text: &str,
    origin: Point<Pixels>,
    font_size: Pixels,
    color: Hsla,
    window: &mut Window,
    cx: &mut App,
) {
    if text.is_empty() {
        return;
    }
    let line = shape(text, font_size, color, window);
    let _ = line.paint(origin, font_size * 1.3, TextAlign::Left, None, window, cx);
}
