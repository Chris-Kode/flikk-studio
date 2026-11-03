//! Timeline geometry: time ↔ x, track rows, ruler ticks, snapping.

use flikk_core::model::{Timeline, TrackKind};
use flikk_core::{Rate, Time, TrackId};
use std::collections::HashMap;

pub const HEADER_WIDTH: f32 = 180.;
pub const RULER_HEIGHT: f32 = 24.;
pub const TOOLBAR_HEIGHT: f32 = 30.;
/// Distance in px within which a drag snaps to a candidate time.
pub const SNAP_PX: f32 = 8.;
/// Distance in px from a clip edge that starts a trim instead of a move.
pub const EDGE_PX: f32 = 6.;
pub const MIN_PX_PER_SEC: f32 = 0.5;
pub const MAX_PX_PER_SEC: f32 = 4000.;
/// Space below the last track, so the lanes never end flush with the panel.
pub const BOTTOM_PAD: f32 = 24.;
/// Track heights a header drag may set.
pub const MIN_ROW_HEIGHT: f32 = 22.;
pub const MAX_ROW_HEIGHT: f32 = 160.;

pub fn row_height(kind: TrackKind) -> f32 {
    match kind {
        TrackKind::Video => 46.,
        TrackKind::Overlay => 36.,
        TrackKind::Audio => 40.,
        TrackKind::Title => 32.,
        TrackKind::Caption => 26.,
    }
}

/// A clip's box inside its lane: 3 px clear above and below.
pub const CLIP_INSET: f32 = 3.;

/// Where a clip's filmstrip sits under its name: offset from the clip's
/// top and height. Short clips run it behind the name.
pub fn filmstrip(clip_height: f32) -> (f32, f32) {
    if clip_height >= 30. {
        (15., clip_height - 16.)
    } else {
        (1., clip_height - 2.)
    }
}

/// Horizontal mapping plus scroll offsets, all in logical px.
#[derive(Clone, Copy, Debug)]
pub struct Viewport {
    pub px_per_sec: f32,
    pub scroll_x: f32,
    pub scroll_y: f32,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            px_per_sec: 40.,
            scroll_x: 0.,
            scroll_y: 0.,
        }
    }
}

impl Viewport {
    /// Lane-local x of timeline time `t`.
    pub fn x(&self, t: Time) -> f32 {
        t.as_secs_f64() as f32 * self.px_per_sec - self.scroll_x
    }

    /// Timeline time at lane-local x.
    pub fn time(&self, x: f32) -> Time {
        Time::from_secs_f64(((x + self.scroll_x) / self.px_per_sec) as f64)
    }

    /// A length in px as time.
    pub fn span(&self, px: f32) -> Time {
        Time::from_secs_f64((px / self.px_per_sec) as f64)
    }

    /// Zoom by `factor`, keeping the time under lane-local `anchor_x` still.
    pub fn zoom_at(&mut self, factor: f32, anchor_x: f32) {
        let t = (anchor_x + self.scroll_x) / self.px_per_sec;
        self.px_per_sec = (self.px_per_sec * factor).clamp(MIN_PX_PER_SEC, MAX_PX_PER_SEC);
        self.scroll_x = (t * self.px_per_sec - anchor_x).max(0.);
    }

    /// Show `duration` across `width` px with a little room on the right.
    pub fn fit(&mut self, duration: Time, width: f32) {
        let secs = duration.as_secs_f64().max(1.) as f32;
        self.px_per_sec = (width * 0.95 / secs).clamp(MIN_PX_PER_SEC, MAX_PX_PER_SEC);
        self.scroll_x = 0.;
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Row {
    pub track: TrackId,
    pub kind: TrackKind,
    /// Top, relative to the first row, before vertical scroll.
    pub y: f32,
    pub height: f32,
}

/// Rows top to bottom; `heights` holds tracks the user resized.
pub fn rows(timeline: &Timeline, heights: &HashMap<TrackId, f32>) -> Vec<Row> {
    let mut y = 0.;
    timeline
        .tracks
        .iter()
        .map(|t| {
            let height = heights
                .get(&t.id)
                .copied()
                .unwrap_or_else(|| row_height(t.kind));
            let row = Row {
                track: t.id,
                kind: t.kind,
                y,
                height,
            };
            y += height;
            row
        })
        .collect()
}

pub fn rows_height(rows: &[Row]) -> f32 {
    rows.last().map(|r| r.y + r.height).unwrap_or(0.)
}

/// The row at lane-local, scrolled `y`.
pub fn row_at(rows: &[Row], y: f32) -> Option<&Row> {
    rows.iter().find(|r| y >= r.y && y < r.y + r.height)
}

/// Ruler steps: a major interval with a label and the minor subdivisions.
pub struct Ticks {
    pub major: Time,
    pub minor: Time,
    /// Labels show frames when the major step is below a second.
    pub frames: bool,
}

pub fn ticks(px_per_sec: f32, rate: Rate) -> Ticks {
    let frame = rate.frame_duration();
    let fps = rate.rounded_fps() as i64;
    let s = |secs: i64| Time::from_secs_f64(secs as f64);
    let mut steps: Vec<(Time, Time, bool)> = Vec::new();
    for f in [1, 2, 5, 10] {
        if f < fps {
            let minor = if f >= 5 { f / 5 } else { 1 };
            steps.push((Time(frame.0 * f), Time(frame.0 * minor), true));
        }
    }
    for (major, minor) in [
        (1, 0),
        (2, 1),
        (5, 1),
        (10, 2),
        (15, 5),
        (30, 5),
        (60, 10),
        (120, 30),
        (300, 60),
        (600, 120),
        (900, 300),
        (1800, 300),
        (3600, 600),
    ] {
        let minor = if minor == 0 {
            Time(frame.0 * (fps / 4).max(1))
        } else {
            s(minor)
        };
        steps.push((s(major), minor, false));
    }
    let min_major_px = 90.;
    let (major, minor, frames) = steps
        .iter()
        .copied()
        .find(|(major, _, _)| major.as_secs_f64() as f32 * px_per_sec >= min_major_px)
        .unwrap_or(*steps.last().unwrap());
    Ticks {
        major,
        minor,
        frames,
    }
}

/// Ruler label: `MM:SS`, `H:MM:SS`, or with `:FF` frames when zoomed in.
pub fn ruler_label(t: Time, rate: Rate, frames: bool) -> String {
    let fps = rate.rounded_fps() as i64;
    let total = t.to_frame(rate).max(0);
    let secs = total / fps;
    let (h, m, s, f) = (secs / 3600, secs / 60 % 60, secs % 60, total % fps);
    let base = if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    };
    if frames {
        format!("{base}:{f:02}")
    } else {
        base
    }
}

/// The candidate nearest to `t` within `threshold`, if any.
pub fn snap(t: Time, candidates: &[Time], threshold: Time) -> Option<Time> {
    candidates
        .iter()
        .copied()
        .filter(|c| (c.0 - t.0).abs() <= threshold.0)
        .min_by_key(|c| (c.0 - t.0).abs())
}
