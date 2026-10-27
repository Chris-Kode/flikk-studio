//! Custom curves: master, red, green and blue on one painted square.
//! Drag a point; double-click to add one; drag it out or right-click to
//! remove it.

use super::editor::{CurveDrag, GradeUi};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;
use koo_core::grade::Curve;
use koo_core::model::Grade;

pub const CHANNELS: [&str; 4] = ["Master", "R", "G", "B"];

/// Pick radius around a point, px.
const HIT: f32 = 8.;
/// Distance outside the square that removes a dragged point, px.
const OUT: f32 = 24.;

pub fn points(g: &Grade, channel: usize) -> &Vec<[f32; 2]> {
    let c = &g.curves;
    [&c.master, &c.red, &c.green, &c.blue][channel]
}

fn points_mut(g: &mut Grade, channel: usize) -> &mut Vec<[f32; 2]> {
    let c = &mut g.curves;
    match channel {
        0 => &mut c.master,
        1 => &mut c.red,
        2 => &mut c.green,
        _ => &mut c.blue,
    }
}

/// Store `points` for `channel`; a bare diagonal is stored as no points.
pub fn set_points(g: &mut Grade, channel: usize, points: &[[f32; 2]]) {
    let identity = points.len() == 2
        && points
            .iter()
            .all(|[x, y]| (x - y).abs() < 1e-4 && (x.abs() < 1e-4 || (x - 1.0).abs() < 1e-4));
    *points_mut(g, channel) = if identity {
        Vec::new()
    } else {
        points.to_vec()
    };
}

/// The edit a drag stands for.
fn drag_edit(d: &CurveDrag) -> impl Fn(&mut Grade) + use<> {
    let mut points = d.points.clone();
    if d.out {
        points.remove(d.index);
    }
    let channel = d.channel;
    move |g: &mut Grade| set_points(g, channel, &points)
}

fn to_px(b: &Bounds<Pixels>, p: [f32; 2]) -> Point<Pixels> {
    point(
        b.origin.x + b.size.width * p[0],
        b.origin.y + b.size.height * (1.0 - p[1]),
    )
}

fn from_px(b: &Bounds<Pixels>, p: Point<Pixels>) -> [f32; 2] {
    [
        ((p.x - b.origin.x) / b.size.width).clamp(0.0, 1.0),
        (1.0 - (p.y - b.origin.y) / b.size.height).clamp(0.0, 1.0),
    ]
}

fn channel_color(channel: usize, cx: &App) -> Hsla {
    let t = cx.theme();
    [t.foreground, t.red, t.green, t.blue][channel]
}

/// The editor square for `ui`'s current channel.
pub fn editor(ui: Entity<GradeUi>, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let (grid, bg) = (theme.border, theme.secondary);
    let colors: Vec<Hsla> = (0..4).map(|c| channel_color(c, cx)).collect();
    let state = ui.clone();
    canvas(
        move |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |bounds, hitbox, window, cx| {
            let (grade, channel, drag) = {
                let ui = state.read(cx);
                let drag = ui
                    .curve_drag
                    .as_ref()
                    .map(|d| (d.points.clone(), d.index, d.out));
                (ui.shown().clone(), ui.channel, drag)
            };
            paint(&bounds, &grade, channel, drag, &colors, grid, bg, window);
            window.set_cursor_style(CursorStyle::Crosshair, &hitbox);
            listen(state.clone(), bounds, hitbox, window);
        },
    )
    .w_full()
    .h(px(200.))
}

#[allow(clippy::too_many_arguments)]
fn paint(
    b: &Bounds<Pixels>,
    grade: &Grade,
    channel: usize,
    drag: Option<(Vec<[f32; 2]>, usize, bool)>,
    colors: &[Hsla],
    grid: Hsla,
    bg: Hsla,
    window: &mut Window,
) {
    window.paint_quad(fill(*b, bg));
    for i in 1..4 {
        let f = i as f32 / 4.0;
        let x = b.origin.x + b.size.width * f;
        let y = b.origin.y + b.size.height * f;
        window.paint_quad(fill(
            Bounds::new(point(x, b.origin.y), size(px(1.), b.size.height)),
            grid,
        ));
        window.paint_quad(fill(
            Bounds::new(point(b.origin.x, y), size(b.size.width, px(1.))),
            grid,
        ));
    }
    stroke(b, &[[0.0, 0.0], [1.0, 1.0]], grid.opacity(0.8), 1., window);
    // Other channels' curves, faint, under this one.
    for c in (0..4).filter(|c| *c != channel) {
        let pts = Curve::with_ends(points(grade, c));
        if let Some(curve) = Curve::new(&pts) {
            stroke(b, &sample(&curve), colors[c].opacity(0.35), 1., window);
        }
    }
    let (pts, active, out) = match drag {
        Some((p, i, out)) => (p, Some(i), out),
        None => (Curve::with_ends(points(grade, channel)), None, false),
    };
    let mut shown = pts.clone();
    if let (Some(i), true) = (active, out) {
        shown.remove(i);
    }
    let line = match Curve::new(&shown) {
        Some(curve) => sample(&curve),
        None => vec![[0.0, 0.0], [1.0, 1.0]],
    };
    let color = colors[channel];
    stroke(b, &line, color, 1.5, window);
    for (i, p) in pts.iter().enumerate() {
        if out && active == Some(i) {
            continue;
        }
        let c = to_px(b, *p);
        let r = px(if active == Some(i) { 5. } else { 4. });
        window.paint_quad(quad(
            Bounds::new(point(c.x - r, c.y - r), size(r * 2., r * 2.)),
            r,
            if active == Some(i) { color } else { bg },
            px(1.5),
            color,
            BorderStyle::default(),
        ));
    }
}

fn sample(curve: &Curve) -> Vec<[f32; 2]> {
    (0..=96)
        .map(|i| {
            let x = i as f32 / 96.0;
            [x, curve.eval(x)]
        })
        .collect()
}

fn stroke(b: &Bounds<Pixels>, line: &[[f32; 2]], color: Hsla, width: f32, window: &mut Window) {
    let mut path = PathBuilder::stroke(px(width));
    for (i, p) in line.iter().enumerate() {
        if i == 0 {
            path.move_to(to_px(b, *p));
        } else {
            path.line_to(to_px(b, *p));
        }
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

fn hit(b: &Bounds<Pixels>, pts: &[[f32; 2]], at: Point<Pixels>) -> Option<usize> {
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

fn listen(ui: Entity<GradeUi>, b: Bounds<Pixels>, hitbox: Hitbox, window: &mut Window) {
    let down = ui.clone();
    window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, cx| {
        if phase != DispatchPhase::Bubble || !hitbox.is_hovered(window) {
            return;
        }
        down.update(cx, |ui, cx| {
            let channel = ui.channel;
            let mut pts = Curve::with_ends(points(ui.shown(), channel));
            let found = hit(&b, &pts, ev.position);
            match (ev.button, found) {
                (MouseButton::Right, Some(i)) => {
                    pts.remove(i);
                    ui.commit(move |g| set_points(g, channel, &pts), cx);
                }
                (MouseButton::Left, Some(index)) => {
                    ui.curve_drag = Some(CurveDrag {
                        channel,
                        points: pts,
                        index,
                        out: false,
                    });
                    cx.notify();
                }
                (MouseButton::Left, None) if ev.click_count >= 2 => {
                    let p = from_px(&b, ev.position);
                    let index = pts.partition_point(|q| q[0] < p[0]);
                    pts.insert(index, p);
                    let d = CurveDrag {
                        channel,
                        points: pts,
                        index,
                        out: false,
                    };
                    ui.preview(drag_edit(&d), cx);
                    ui.curve_drag = Some(d);
                }
                _ => return,
            }
            cx.stop_propagation();
        });
    });
    let moved = ui.clone();
    window.on_mouse_event(move |ev: &MouseMoveEvent, _, _, cx| {
        if moved.read(cx).curve_drag.is_none() {
            return;
        }
        moved.update(cx, |ui, cx| {
            let Some(d) = ui.curve_drag.as_mut() else {
                return;
            };
            let i = d.index;
            let n = d.points.len();
            let p = from_px(&b, ev.position);
            let lo = if i == 0 {
                0.0
            } else {
                d.points[i - 1][0] + 0.01
            };
            let hi = if i + 1 == n {
                1.0
            } else {
                d.points[i + 1][0] - 0.01
            };
            d.points[i] = [p[0].clamp(lo, hi.max(lo)), p[1]];
            let outside = ev.position.x < b.origin.x - px(OUT)
                || ev.position.x > b.origin.x + b.size.width + px(OUT)
                || ev.position.y < b.origin.y - px(OUT)
                || ev.position.y > b.origin.y + b.size.height + px(OUT);
            d.out = outside && n > 2;
            let edit = drag_edit(d);
            ui.preview(edit, cx);
            cx.notify();
        });
    });
    window.on_mouse_event(move |ev: &MouseUpEvent, _, _, cx| {
        if ev.button != MouseButton::Left || ui.read(cx).curve_drag.is_none() {
            return;
        }
        ui.update(cx, |ui, cx| {
            if let Some(d) = ui.curve_drag.take() {
                ui.commit(drag_edit(&d), cx);
            }
            cx.notify();
        });
    });
}
