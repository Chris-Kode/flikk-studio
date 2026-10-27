//! Chat references for timeline objects, in the token grammar of
//! TECHNICAL_DESIGN §8: token text as the agent reads it, chip label.

use super::layout::{RULER_HEIGHT, Row};
use crate::ui::RefDrag;
use gpui_kit::*;
use koo_core::command::Edge;
use koo_core::model::{Clip, Marker, Project, Track};
use koo_core::{ClipId, Time, TimeRange};

pub type Ref = (String, SharedString);

pub fn clip(clip: &Clip, project: &Project) -> Ref {
    (
        format!("@clip {}", clip.id),
        clip.display_name(project).into(),
    )
}

pub fn marker(marker: &Marker) -> Ref {
    let label = match marker.note.trim() {
        "" => format!("Marker {}", clock(marker.range.start)),
        note => note.to_owned(),
    };
    (format!("@marker {}", marker.id), label.into())
}

pub fn track(track: &Track) -> Ref {
    (format!("@track {}", track.id), track.name.clone().into())
}

pub fn transition(clip: ClipId, edge: Edge, project: &Project) -> Ref {
    let side = match edge {
        Edge::Start => "in",
        Edge::End => "out",
    };
    let label = project
        .timeline
        .clip(clip)
        .and_then(|(_, c)| match edge {
            Edge::Start => c.transition_in.as_ref(),
            Edge::End => c.transition_out.as_ref(),
        })
        .and_then(|t| koo_core::fx::asset(project, &t.fx.fx))
        .map_or_else(|| "Transition".to_owned(), |a| a.name.clone());
    (format!("@transition {clip}/{side}"), label.into())
}

pub fn range(r: TimeRange) -> Ref {
    (
        format!(
            "@range {:.3}-{:.3}",
            r.start.as_secs_f64(),
            r.end.as_secs_f64()
        ),
        format!("{}–{}", clock(r.start), clock(r.end)).into(),
    )
}

pub fn time(t: Time) -> Ref {
    (format!("@time {:.3}", t.as_secs_f64()), clock(t).into())
}

/// `M:SS.s`, the chip label for a time.
fn clock(t: Time) -> String {
    let tenths = (t.as_secs_f64().max(0.) * 10.).round() as i64;
    let (m, s, f) = (tenths / 600, tenths / 10 % 60, tenths % 10);
    format!("{m}:{s:02}.{f}")
}

impl super::TimelinePanel {
    /// While Alt is held: transparent drag sources over visible clips,
    /// transitions, markers and the in/out range, so an Alt-drag carries a
    /// reference to the chat instead of editing. GPUI starts drags only
    /// from an element's `on_drag`, hence real elements over the canvas.
    pub(super) fn ref_overlays(&self, cx: &App) -> Vec<AnyElement> {
        if !self.alt || self.drag.is_some() {
            return vec![];
        }
        let b = self.bounds.get();
        let (width, height) = (f32::from(b.size.width), f32::from(b.size.height));
        if width <= 0. {
            return vec![];
        }
        let state = self.state.read(cx);
        let project = state.bus.project();
        let tl = &project.timeline;
        let vp = self.viewport;
        let visible = TimeRange::new(vp.time(0.), vp.time(width));
        let selection = &state.selection.clips;
        let mut out = Vec::new();
        let mut zone = |key: String, x: (f32, f32), y: (f32, f32), floor: f32, refs: Vec<Ref>| {
            let (x0, x1) = (x.0.max(0.), x.1.min(width));
            let (y0, y1) = (y.0.max(floor), y.1.min(height));
            if x1 - x0 < 2. || y1 - y0 < 2. || refs.is_empty() {
                return;
            }
            out.push(
                div()
                    .id(ElementId::Name(key.into()))
                    .absolute()
                    .left(px(x0))
                    .top(px(y0))
                    .w(px(x1 - x0))
                    .h(px(y1 - y0))
                    .cursor(CursorStyle::DragCopy)
                    .on_drag(RefDrag { refs }, |drag, _, _, cx| cx.new(|_| drag.clone()))
                    .into_any_element(),
            );
        };

        let lane_y = |row: &Row| RULER_HEIGHT + row.y - vp.scroll_y;
        let rows = self.rows(tl);
        let mut budget = 400;
        for (row, track) in rows.iter().zip(&tl.tracks) {
            for c in track
                .clips
                .iter()
                .filter(|c| c.end() >= visible.start && c.start <= visible.end)
            {
                if budget == 0 {
                    break;
                }
                budget -= 1;
                // A selected clip carries the whole selection.
                let refs = if selection.len() > 1 && selection.contains(&c.id) {
                    selection
                        .iter()
                        .filter_map(|id| tl.clip(*id))
                        .map(|(_, c)| clip(c, project))
                        .collect()
                } else {
                    vec![clip(c, project)]
                };
                let top = lane_y(row);
                zone(
                    format!("ref-clip-{}", c.id),
                    (vp.x(c.start), vp.x(c.end())),
                    (top + 2., top + row.height - 2.),
                    RULER_HEIGHT,
                    refs,
                );
            }
            for t in super::fx::placed(track) {
                let (a, b) = super::transition_span(&vp, t.range);
                let top = lane_y(row);
                zone(
                    format!("ref-transition-{}-{:?}", t.owner.0, t.owner.1),
                    (a, b),
                    (top + 2., top + row.height - 2.),
                    RULER_HEIGHT,
                    vec![transition(t.owner.0, t.owner.1, project)],
                );
            }
        }
        if let Some(r) = state.in_out.filter(|r| !r.is_empty()) {
            zone(
                "ref-in-out".into(),
                (vp.x(r.start), vp.x(r.end)),
                (RULER_HEIGHT - 7., RULER_HEIGHT),
                0.,
                vec![range(r)],
            );
        }
        for m in &tl.markers {
            let a = vp.x(m.range.start);
            zone(
                format!("ref-marker-{}", m.id),
                (a - 5., vp.x(m.range.end).max(a) + 5.),
                (RULER_HEIGHT - 14., RULER_HEIGHT - 2.),
                0.,
                vec![marker(m)],
            );
        }
        out
    }
}
