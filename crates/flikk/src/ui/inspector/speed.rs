//! A media clip's Speed tab: its speed over the clip as a graph (drag a
//! point, double-click to add one, right-click to remove it), ready-made
//! ramps, keeping the clip's length, how slowed frames are drawn and what
//! ramped audio does. A ramp applies to the clip and its linked audio.

use super::{group, label, row, wide};
use crate::state::EditorState;
use crate::ui::kit::theme as t;
use crate::ui::kit::{Button, SectionHeader, Segmented, Switch, h_flex, v_flex};
use flikk_core::ClipId;
use flikk_core::command::Command;
use flikk_core::model::Clip;
use flikk_core::speed::{Interpolation, MAX_SPEED, MIN_SPEED, RampAudio, SpeedPreset};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// Pick radius around a point, px.
const HIT: f32 = 8.;
/// Slowest and fastest speed the graph shows (log scale).
const LOW: f64 = 0.1;
const HIGH: f64 = 10.;
/// Speeds marked across the graph.
const GRID: [f64; 5] = [0.25, 0.5, 1., 2., 4.];
const HEIGHT: f32 = 140.;

pub struct SpeedRamp {
    state: Entity<EditorState>,
    clip: Option<ClipId>,
    /// The clip's linked audio, ramped with it.
    audio: Option<ClipId>,
    ripple: bool,
    keep_length: bool,
    /// The group is unfolded.
    open: bool,
    drag: Option<Drag>,
    _observe: Subscription,
}

/// A point being dragged: every point (position 0..1, speed) and which.
#[derive(Clone)]
struct Drag {
    points: Vec<[f64; 2]>,
    index: usize,
}

impl SpeedRamp {
    pub fn new(state: Entity<EditorState>, cx: &mut Context<Self>) -> Self {
        let _observe = cx.observe(&state, |_, _, cx| cx.notify());
        Self {
            state,
            clip: None,
            audio: None,
            ripple: false,
            keep_length: false,
            open: true,
            drag: None,
            _observe,
        }
    }

    pub fn set_clip(&mut self, clip: ClipId, audio: Option<ClipId>, ripple: bool) {
        if self.clip != Some(clip) {
            self.drag = None;
        }
        self.clip = Some(clip);
        self.audio = audio;
        self.ripple = ripple;
    }

    fn clip<'a>(&self, cx: &'a App) -> Option<&'a Clip> {
        let id = self.clip?;
        self.state
            .read(cx)
            .bus
            .project()
            .timeline
            .clip(id)
            .map(|(_, c)| c)
    }

    /// The clip's speed as points over it; flat at its speed without a
    /// ramp.
    fn points(clip: &Clip) -> Vec<[f64; 2]> {
        let d = clip.duration.as_secs_f64().max(1e-9);
        if clip.is_ramped() {
            clip.retime
                .keys
                .iter()
                .map(|k| [(k.at.as_secs_f64() / d).clamp(0., 1.), k.speed])
                .collect()
        } else {
            vec![[0., clip.speed], [1., clip.speed]]
        }
    }

    fn ramp(&self, points: Vec<[f64; 2]>, keep_length: bool, cx: &mut Context<Self>) {
        let Some(clip) = self.clip else {
            return;
        };
        let ripple = self.ripple;
        let commands: Vec<Command> = std::iter::once(clip)
            .chain(self.audio)
            .map(|clip| Command::SetSpeedRamp {
                clip,
                points: points.clone(),
                keep_length,
                ripple,
            })
            .collect();
        self.state.update(cx, |s, cx| {
            s.edit_batch(commands, "set_speed_ramp", cx);
        });
    }

    fn retime(
        &self,
        clip: ClipId,
        interpolation: Option<Interpolation>,
        audio: Option<RampAudio>,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| {
            s.edit(
                Command::SetRetime {
                    clip,
                    interpolation,
                    audio,
                },
                cx,
            );
            if interpolation == Some(Interpolation::OpticalFlow) {
                s.optical_flow_picked(cx);
            }
        });
    }
}

fn y_of(speed: f64) -> f32 {
    ((speed.clamp(LOW, HIGH).log10() - LOW.log10()) / (HIGH.log10() - LOW.log10())) as f32
}

fn speed_of(y: f32) -> f64 {
    let s = 10f64.powf(LOW.log10() + y.clamp(0., 1.) as f64 * (HIGH.log10() - LOW.log10()));
    // Snap near 1×, so a flat stretch is easy to set.
    if (s - 1.).abs() < 0.04 {
        1.
    } else {
        s.clamp(MIN_SPEED.max(LOW), MAX_SPEED.min(HIGH))
    }
}

fn to_px(b: &Bounds<Pixels>, p: [f64; 2]) -> Point<Pixels> {
    point(
        b.origin.x + b.size.width * p[0] as f32,
        b.origin.y + b.size.height * (1. - y_of(p[1])),
    )
}

fn hit(b: &Bounds<Pixels>, pts: &[[f64; 2]], at: Point<Pixels>) -> Option<usize> {
    pts.iter()
        .enumerate()
        .map(|(i, p)| {
            let c = to_px(b, *p);
            (i, f32::from(c.x - at.x).hypot(f32::from(c.y - at.y)))
        })
        .filter(|(_, d)| *d <= HIT)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

struct Colors {
    bg: Hsla,
    grid: Hsla,
    line: Hsla,
    text: Hsla,
    playhead: Hsla,
}

#[allow(clippy::too_many_arguments)]
fn paint(
    b: &Bounds<Pixels>,
    pts: &[[f64; 2]],
    active: Option<usize>,
    playhead: Option<f32>,
    c: &Colors,
    window: &mut Window,
    cx: &mut App,
) {
    window.paint_quad(fill(*b, c.bg));
    for s in GRID {
        let y = b.origin.y + b.size.height * (1. - y_of(s));
        let color = if s == 1. { c.grid } else { c.grid.opacity(0.5) };
        window.paint_quad(fill(
            Bounds::new(point(b.origin.x, y), size(b.size.width, px(1.))),
            color,
        ));
        paint_text(
            &format!("{s}×"),
            point(b.origin.x + px(4.), y - px(13.)),
            c.text,
            window,
            cx,
        );
    }
    if let Some(x) = playhead {
        window.paint_quad(fill(
            Bounds::new(
                point(b.origin.x + b.size.width * x, b.origin.y),
                size(px(1.), b.size.height),
            ),
            c.playhead,
        ));
    }
    let mut path = PathBuilder::stroke(px(1.5));
    path.move_to(to_px(b, [0., pts[0][1]]));
    for p in pts {
        path.line_to(to_px(b, *p));
    }
    path.line_to(to_px(b, [1., pts[pts.len() - 1][1]]));
    if let Ok(path) = path.build() {
        window.paint_path(path, c.line);
    }
    for (i, p) in pts.iter().enumerate() {
        let at = to_px(b, *p);
        let on = active == Some(i);
        let r = px(if on { 5. } else { 4. });
        window.paint_quad(quad(
            Bounds::new(point(at.x - r, at.y - r), size(r * 2., r * 2.)),
            r,
            if on { c.line } else { c.bg },
            px(1.5),
            c.line,
            BorderStyle::default(),
        ));
    }
    if let Some(p) = active.and_then(|i| pts.get(i)) {
        let at = to_px(b, *p);
        let x = (at.x + px(8.)).min(b.origin.x + b.size.width - px(40.));
        let y = (at.y - px(20.)).max(b.origin.y + px(2.));
        let shown = format!("{}%", (p[1] * 100.).round());
        paint_text(&shown, point(x, y), c.line, window, cx);
    }
}

fn paint_text(text: &str, at: Point<Pixels>, color: Hsla, window: &mut Window, cx: &mut App) {
    let run = TextRun {
        len: text.len(),
        font: font(t::mono()),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = window.text_system().shape_line(
        SharedString::from(text.to_string()),
        px(10.),
        &[run],
        None,
    );
    let _ = line.paint(at, px(13.), TextAlign::Left, None, window, cx);
}

/// The graph: the clip's speed (or the point being dragged), the playhead.
fn graph(ramp: Entity<SpeedRamp>) -> impl IntoElement {
    let colors = Colors {
        bg: t::G.into(),
        grid: t::LN.into(),
        line: t::AC.into(),
        text: t::T3.into(),
        playhead: t::PH.into(),
    };
    let state = ramp.clone();
    canvas(
        move |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |bounds, hitbox, window, cx| {
            let (pts, active, playhead) = {
                let r = state.read(cx);
                let Some(clip) = r.clip(cx) else {
                    return;
                };
                let playhead = r.state.read(cx).playhead;
                let at = clip.range().contains(playhead).then(|| {
                    ((playhead - clip.start).as_secs_f64() / clip.duration.as_secs_f64().max(1e-9))
                        as f32
                });
                match &r.drag {
                    Some(d) => (d.points.clone(), Some(d.index), at),
                    None => (SpeedRamp::points(clip), None, at),
                }
            };
            paint(&bounds, &pts, active, playhead, &colors, window, cx);
            window.set_cursor_style(CursorStyle::Crosshair, &hitbox);
            listen(state.clone(), bounds, hitbox, window);
        },
    )
    .w_full()
    .h(px(HEIGHT))
}

fn listen(ramp: Entity<SpeedRamp>, b: Bounds<Pixels>, hitbox: Hitbox, window: &mut Window) {
    let down = ramp.clone();
    window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, cx| {
        if phase != DispatchPhase::Bubble || !hitbox.is_hovered(window) {
            return;
        }
        down.update(cx, |r, cx| {
            let Some(clip) = r.clip(cx) else {
                return;
            };
            let mut pts = SpeedRamp::points(clip);
            let keep = r.keep_length;
            match (ev.button, hit(&b, &pts, ev.position)) {
                (MouseButton::Right, Some(i)) if pts.len() > 2 => {
                    pts.remove(i);
                    r.ramp(pts, keep, cx);
                }
                (MouseButton::Left, Some(index)) => {
                    r.drag = Some(Drag { points: pts, index });
                    cx.notify();
                }
                (MouseButton::Left, None) if ev.click_count >= 2 => {
                    let u = f64::from((ev.position.x - b.origin.x) / b.size.width).clamp(0., 1.);
                    let y = 1. - (ev.position.y - b.origin.y) / b.size.height;
                    let index = pts.partition_point(|q| q[0] < u);
                    pts.insert(index, [u, speed_of(y)]);
                    r.drag = Some(Drag { points: pts, index });
                    cx.notify();
                }
                _ => return,
            }
            cx.stop_propagation();
        });
    });
    let moved = ramp.clone();
    window.on_mouse_event(move |ev: &MouseMoveEvent, _, _, cx| {
        if moved.read(cx).drag.is_none() {
            return;
        }
        moved.update(cx, |r, cx| {
            let Some(d) = r.drag.as_mut() else {
                return;
            };
            let (i, n) = (d.index, d.points.len());
            let u = f64::from((ev.position.x - b.origin.x) / b.size.width);
            let y = 1. - (ev.position.y - b.origin.y) / b.size.height;
            // The ends stay at the clip's ends; the others between their
            // neighbours.
            let u = if i == 0 {
                0.
            } else if i + 1 == n {
                1.
            } else {
                u.clamp(
                    d.points[i - 1][0] + 0.01,
                    (d.points[i + 1][0] - 0.01).max(d.points[i - 1][0] + 0.01),
                )
            };
            d.points[i] = [u, speed_of(y)];
            cx.notify();
        });
    });
    window.on_mouse_event(move |ev: &MouseUpEvent, _, _, cx| {
        if ev.button != MouseButton::Left || ramp.read(cx).drag.is_none() {
            return;
        }
        ramp.update(cx, |r, cx| {
            if let Some(d) = r.drag.take() {
                let keep = r.keep_length;
                r.ramp(d.points, keep, cx);
            }
            cx.notify();
        });
    });
}

impl Render for SpeedRamp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(clip) = self.clip(cx).cloned() else {
            return v_flex();
        };
        let header = SectionHeader::new("ramp", "Ramp")
            .open(self.open)
            .on_toggle(cx.listener(|this, open: &bool, _, cx| {
                this.open = *open;
                cx.notify();
            }))
            .when(clip.is_ramped(), |h| {
                h.child(
                    Button::new("ramp-none")
                        .ghost()
                        .xsmall()
                        .label("None")
                        .tooltip("Remove the ramp")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.ramp(Vec::new(), true, cx);
                        })),
                )
            });
        if !self.open {
            return group(header, Vec::<Div>::new());
        }
        let audio = self.audio.and_then(|id| {
            self.state
                .read(cx)
                .bus
                .project()
                .timeline
                .clip(id)
                .map(|(_, c)| c.clone())
        });
        let base = clip.speed;
        let presets = h_flex()
            .flex_wrap()
            .gap(px(4.))
            .children(SpeedPreset::ALL.into_iter().map(|p| {
                Button::new(p.key())
                    .xsmall()
                    .label(p.label())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.ramp(p.points(base), this.keep_length, cx);
                    }))
            }));
        let keep = row(
            label("Keep length"),
            Switch::new("ramp-keep-length")
                .checked(self.keep_length)
                .on_click(cx.listener(|this, on: &bool, _, cx| {
                    this.keep_length = *on;
                    cx.notify();
                })),
            None,
        );
        let id = clip.id;
        let current = clip.retime.interpolation;
        let modes = Segmented::new("ramp-frames")
            .compact()
            .fill()
            .items(Interpolation::ALL.map(|m| m.label()))
            .selected(
                Interpolation::ALL
                    .iter()
                    .position(|m| *m == current)
                    .unwrap_or(0),
            )
            .on_click(cx.listener(move |this, ix: &usize, _, cx| {
                if let Some(m) = Interpolation::ALL.get(*ix).copied()
                    && m != current
                {
                    this.retime(id, Some(m), None, cx);
                }
            }));
        let sound = audio.filter(|a| a.is_ramped()).map(|a| {
            let muted = a.retime.audio == RampAudio::Mute;
            let audio_id = a.id;
            row(
                label("Mute audio"),
                Switch::new("ramp-mute")
                    .checked(muted)
                    .on_click(cx.listener(move |this, on: &bool, _, cx| {
                        let audio = if *on {
                            RampAudio::Mute
                        } else {
                            RampAudio::Stretch
                        };
                        this.retime(audio_id, None, Some(audio), cx);
                    })),
                None,
            )
        });
        let graph = div()
            .rounded(px(6.))
            .border_1()
            .border_color(t::LN)
            .overflow_hidden()
            .child(graph(cx.entity()));
        let rows: Vec<Div> = [wide(graph), wide(presets), keep, wide(modes)]
            .into_iter()
            .chain(sound)
            .collect();
        group(header, rows)
    }
}
