//! Lift, gamma, gain and offset as color wheels: the puck moves the
//! channel balance in the vectorscope's plane (toward red on the wheel is
//! toward red on the scope) at constant luma; the bar below moves all
//! three channels. Double-click either to reset it.

use super::editor::{GradeUi, Layout};
use super::fields::{WHEELS, Wheel};
use crate::ui::kit::theme as t;
use crate::ui::kit::{h_flex, v_flex};
use flikk_core::grade::LUMA;
use flikk_core::model::Grade;
use gpui_kit::*;

/// Chroma at the rim, as a fraction of the wheel's master range.
const RIM: f32 = 0.4;
/// Gap between the wheel and its master bar, and the bar's thickness.
const GAP: f32 = 7.;
const BAR: f32 = 3.;
/// The hue ring's inner radius, as a fraction of the wheel's.
const RING: f32 = 0.72;

/// Channel deltas from luma `y` and chroma `(cb, cr)` (Rec. 709).
fn rgb(y: f32, cb: f32, cr: f32) -> [f32; 3] {
    let r = y + 1.5748 * cr;
    let b = y + 1.8556 * cb;
    let g = (y - LUMA[0] * r - LUMA[2] * b) / LUMA[1];
    [r, g, b]
}

/// Luma, Cb and Cr of channel deltas.
fn ycc(d: [f32; 3]) -> (f32, f32, f32) {
    let y = LUMA[0] * d[0] + LUMA[1] * d[1] + LUMA[2] * d[2];
    (y, (d[2] - y) / 1.8556, (d[0] - y) / 1.5748)
}

/// Puck position in the unit disc (y up) and master, for `w`'s values.
fn puck(w: Wheel, v: [f32; 3]) -> (f32, f32, f32) {
    let (y, cb, cr) = ycc(v.map(|c| c - w.neutral()));
    let rim = RIM * w.range();
    (cb / rim, cr / rim, y)
}

fn values(w: Wheel, x: f32, y: f32, master: f32) -> [f32; 3] {
    let len = x.hypot(y);
    let (x, y) = if len > 1.0 {
        (x / len, y / len)
    } else {
        (x, y)
    };
    let rim = RIM * w.range();
    rgb(master, x * rim, y * rim).map(|d| d + w.neutral())
}

/// A master value as shown under its wheel.
fn master_text(m: f32) -> String {
    let s = format!("{:.2}", m.abs());
    if s == "0.00" {
        s
    } else if m < 0. {
        format!("\u{2212}{s}")
    } else {
        format!("+{s}")
    }
}

/// All four wheels in a row: name, wheel and master bar, master value.
pub fn wheels(ui: Entity<GradeUi>, layout: Layout, cx: &App) -> impl IntoElement {
    let g = ui.read(cx).shown().clone();
    let size = layout.wheel_size();
    h_flex()
        .flex_none()
        .px(px(6.))
        .pt(px(6.))
        .pb(px(10.))
        .children(WHEELS.iter().enumerate().map(|(i, w)| {
            let v = w.get(&g);
            let changed = v.iter().any(|c| (c - w.neutral()).abs() > 1e-4);
            let (_, _, master) = puck(*w, v);
            v_flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap(px(4.))
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(if changed { t::T1 } else { t::T2 })
                        .child(w.label()),
                )
                .child(wheel(ui.clone(), i, &g, size))
                .child(
                    div()
                        .font_family(t::mono())
                        .text_size(px(10.))
                        .text_color(t::T3)
                        .child(master_text(master)),
                )
        }))
}

fn wheel(ui: Entity<GradeUi>, index: usize, g: &Grade, diameter: f32) -> impl IntoElement {
    let w = WHEELS[index];
    let (px_, py_, master) = puck(w, w.get(g));
    canvas(
        move |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |bounds, hitbox, window, _| {
            let r = diameter / 2.0;
            let c = point(
                bounds.origin.x + bounds.size.width / 2.0,
                bounds.origin.y + px(r),
            );
            // Hue ring: each segment shows the balance its direction gives.
            let (inner, outer) = (r * RING, r);
            let n = 72;
            for k in 0..n {
                let a0 = k as f32 / n as f32 * std::f32::consts::TAU;
                let a1 = (k + 1) as f32 / n as f32 * std::f32::consts::TAU + 0.01;
                let am = (a0 + a1) / 2.0;
                let d = rgb(0.0, am.cos() * 0.4, am.sin() * 0.4);
                let color: Hsla = Rgba {
                    r: (0.55 + d[0]).clamp(0.0, 1.0),
                    g: (0.55 + d[1]).clamp(0.0, 1.0),
                    b: (0.55 + d[2]).clamp(0.0, 1.0),
                    a: 1.0,
                }
                .into();
                let at = |a: f32, rad: f32| point(c.x + px(a.cos() * rad), c.y - px(a.sin() * rad));
                let mut p = PathBuilder::fill();
                p.add_polygon(
                    &[at(a0, outer), at(a1, outer), at(a1, inner), at(a0, inner)],
                    true,
                );
                if let Ok(p) = p.build() {
                    window.paint_path(p, color);
                }
            }
            let ir = px(inner);
            window.paint_quad(quad(
                Bounds::new(point(c.x - ir, c.y - ir), size(ir * 2., ir * 2.)),
                ir,
                t::R1,
                px(1.),
                t::alpha(t::ON_AC, 0.12),
                BorderStyle::default(),
            ));
            let hair = t::alpha(t::ON_AC, 0.08);
            let arm = ir - px(4.);
            window.paint_quad(fill(
                Bounds::new(point(c.x - arm, c.y), size(arm * 2., px(1.))),
                hair,
            ));
            window.paint_quad(fill(
                Bounds::new(point(c.x, c.y - arm), size(px(1.), arm * 2.)),
                hair,
            ));
            let pr = px(4.5);
            let pc = point(c.x + ir * px_.clamp(-1., 1.), c.y - ir * py_.clamp(-1., 1.));
            window.paint_quad(quad(
                Bounds::new(
                    point(pc.x - pr - px(1.), pc.y - pr - px(1.)),
                    size((pr + px(1.)) * 2., (pr + px(1.)) * 2.),
                ),
                pr + px(1.),
                transparent_black(),
                px(1.),
                t::alpha(t::hex(0x000000), 0.6),
                BorderStyle::default(),
            ));
            window.paint_quad(quad(
                Bounds::new(point(pc.x - pr, pc.y - pr), size(pr * 2., pr * 2.)),
                pr,
                transparent_black(),
                px(1.5),
                t::T1,
                BorderStyle::default(),
            ));
            // Master bar: a centre tick and the offset from it.
            let bar = Bounds::new(
                point(c.x - px(r), bounds.origin.y + px(diameter + GAP)),
                size(px(diameter), px(BAR)),
            );
            window.paint_quad(fill(bar, t::R1).corner_radii(px(BAR / 2.)));
            let f = (master / w.range()).clamp(-1., 1.);
            let mid = bar.origin.x + bar.size.width / 2.0;
            let end = mid + bar.size.width / 2.0 * f;
            let (x0, x1) = if end < mid { (end, mid) } else { (mid, end) };
            if x1 - x0 > px(0.5) {
                window.paint_quad(
                    fill(
                        Bounds::new(point(x0, bar.origin.y), size(x1 - x0, px(BAR))),
                        t::AC,
                    )
                    .corner_radii(px(BAR / 2.)),
                );
            }
            window.paint_quad(fill(
                Bounds::new(
                    point(mid, bar.origin.y - px(2.)),
                    size(px(1.), px(BAR + 4.)),
                ),
                t::alpha(t::ON_AC, 0.19),
            ));
            listen(ui.clone(), index, c, ir, bar, hitbox, window);
        },
    )
    .w_full()
    .h(px(diameter + GAP + BAR + 2.))
}

fn listen(
    ui: Entity<GradeUi>,
    index: usize,
    c: Point<Pixels>,
    radius: Pixels,
    bar: Bounds<Pixels>,
    hitbox: Hitbox,
    window: &mut Window,
) {
    let w = WHEELS[index];
    // The edit for the pointer at `p`, dragging the puck or the bar.
    let edit = move |g: &Grade, p: Point<Pixels>, master_bar: bool| {
        let (x, y, m) = puck(w, w.get(g));
        let v = if master_bar {
            let f = ((p.x - bar.origin.x) / bar.size.width * 2.0 - 1.0).clamp(-1.0, 1.0);
            values(w, x, y, f * w.range())
        } else {
            values(w, (p.x - c.x) / radius, (c.y - p.y) / radius, m)
        };
        move |g: &mut Grade| w.set(g, v)
    };
    let down = ui.clone();
    window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, cx| {
        if phase != DispatchPhase::Bubble
            || ev.button != MouseButton::Left
            || !hitbox.is_hovered(window)
        {
            return;
        }
        let p = ev.position;
        let outer = f32::from(radius) / RING;
        let on_wheel = f32::from(p.x - c.x).hypot(f32::from(p.y - c.y)) <= outer + 4.0;
        let on_bar = p.y >= bar.origin.y - px(6.) && p.y <= bar.origin.y + bar.size.height + px(6.);
        if !on_wheel && !on_bar {
            return;
        }
        down.update(cx, |ui, cx| {
            if ev.click_count >= 2 {
                let (x, y, m) = puck(w, w.get(ui.shown()));
                let v = if on_bar {
                    values(w, x, y, 0.0)
                } else {
                    values(w, 0.0, 0.0, m)
                };
                ui.commit(move |g| w.set(g, v), cx);
            } else {
                ui.wheel_drag = Some((index, on_bar));
                let e = edit(ui.shown(), p, on_bar);
                ui.preview(e, cx);
            }
            cx.notify();
        });
        cx.stop_propagation();
    });
    let moved = ui.clone();
    window.on_mouse_event(move |ev: &MouseMoveEvent, _, _, cx| {
        let Some((i, on_bar)) = moved.read(cx).wheel_drag else {
            return;
        };
        if i != index {
            return;
        }
        moved.update(cx, |ui, cx| {
            // The release was missed: the drag ends uncommitted.
            if ev.pressed_button != Some(MouseButton::Left) {
                ui.cancel_drag(cx);
                return;
            }
            let e = edit(ui.shown(), ev.position, on_bar);
            ui.preview(e, cx);
            cx.notify();
        });
    });
    window.on_mouse_event(move |ev: &MouseUpEvent, _, _, cx| {
        let Some((i, on_bar)) = ui.read(cx).wheel_drag else {
            return;
        };
        if i != index || ev.button != MouseButton::Left {
            return;
        }
        ui.update(cx, |ui, cx| {
            let e = edit(ui.shown(), ev.position, on_bar);
            ui.wheel_drag = None;
            ui.commit(e, cx);
            cx.notify();
        });
    });
}
