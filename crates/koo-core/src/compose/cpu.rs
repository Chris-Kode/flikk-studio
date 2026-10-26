//! The CPU reference for [`crate::services::FxService::composite`], used
//! when there is no GPU engine; `koo-gpu` implements the same arithmetic.
//!
//! For every canvas pixel centre, the layer's picture is found through the
//! inverse of its placement (scale, clockwise rotation, flip), cut by its
//! crop, and sampled bilinearly from premultiplied texels (clamped at the
//! edges, transparent outside the picture). The sample, straight again and
//! with alpha × opacity, is blended onto the canvas with the W3C
//! compositing formula: `Co = αs(1−αb)Cs + αsαb·B(Cb, Cs) + (1−αs)αb·Cb`,
//! `αo = αs + αb(1−αs)`, where `B` is the separable blend mode on straight
//! sRGB values. Over an opaque canvas this is `mix(Cb, B(Cb, Cs), αs)`.

use crate::model::BlendMode;
use crate::services::{CompositeLayer, Frame, Placement};
use std::sync::Arc;

/// One channel of `mode` with backdrop `b` and source `s` (straight, 0..1).
pub fn blend_channel(mode: BlendMode, b: f32, s: f32) -> f32 {
    match mode {
        BlendMode::Normal => s,
        BlendMode::Add => (b + s).min(1.0),
        BlendMode::Screen => b + s - b * s,
        BlendMode::Multiply => b * s,
        BlendMode::Overlay => hard_light(s, b),
        BlendMode::SoftLight => {
            if s <= 0.5 {
                b - (1.0 - 2.0 * s) * b * (1.0 - b)
            } else {
                let d = if b <= 0.25 {
                    ((16.0 * b - 12.0) * b + 4.0) * b
                } else {
                    b.sqrt()
                };
                b + (2.0 * s - 1.0) * (d - b)
            }
        }
        BlendMode::HardLight => hard_light(b, s),
        BlendMode::Lighten => b.max(s),
        BlendMode::Darken => b.min(s),
        BlendMode::Difference => (b - s).abs(),
    }
}

fn hard_light(b: f32, s: f32) -> f32 {
    if s <= 0.5 {
        2.0 * b * s
    } else {
        1.0 - 2.0 * (1.0 - b) * (1.0 - s)
    }
}

/// A placement prepared for sampling.
struct Map<'a> {
    frame: &'a Frame,
    center: [f32; 2],
    sin: f32,
    cos: f32,
    inv: [f32; 2],
    crop: [f32; 4],
    flip: [bool; 2],
    opacity: f32,
    blend: BlendMode,
    /// Canvas pixels the picture can touch: x0, y0, x1, y1 (exclusive).
    bounds: [u32; 4],
}

impl<'a> Map<'a> {
    fn new(layer: &CompositeLayer<'a>, width: u32, height: u32) -> Option<Self> {
        let p: &Placement = &layer.placement;
        let frame = layer.frame;
        let opacity = p.opacity.clamp(0.0, 1.0);
        if p.size[0] <= 0.0 || p.size[1] <= 0.0 || opacity <= 0.0 {
            return None;
        }
        if frame.width == 0 || frame.height == 0 {
            return None;
        }
        let (sin, cos) = p.rotation_deg.to_radians().sin_cos();
        let (hw, hh) = (p.size[0] / 2.0, p.size[1] / 2.0);
        let ex = hw * cos.abs() + hh * sin.abs();
        let ey = hw * sin.abs() + hh * cos.abs();
        let span = |c: f32, e: f32, max: u32| {
            let lo = (c - e - 1.0).floor().clamp(0.0, max as f32) as u32;
            let hi = (c + e + 1.0).ceil().clamp(0.0, max as f32) as u32;
            (lo, hi)
        };
        let (x0, x1) = span(p.center[0], ex, width);
        let (y0, y1) = span(p.center[1], ey, height);
        if x0 >= x1 || y0 >= y1 {
            return None;
        }
        Some(Self {
            frame,
            center: p.center,
            sin,
            cos,
            inv: [1.0 / p.size[0], 1.0 / p.size[1]],
            crop: p.crop.map(|c| c.clamp(0.0, 1.0)),
            flip: p.flip,
            opacity,
            blend: p.blend,
            bounds: [x0, y0, x1, y1],
        })
    }

    /// Premultiplied sample at canvas point (`x`, `y`), alpha × opacity.
    fn sample(&self, x: f32, y: f32) -> Option<[f32; 4]> {
        let (dx, dy) = (x - self.center[0], y - self.center[1]);
        let lx = dx * self.cos + dy * self.sin;
        let ly = -dx * self.sin + dy * self.cos;
        let mut u = lx * self.inv[0] + 0.5;
        let mut v = ly * self.inv[1] + 0.5;
        if self.flip[0] {
            u = 1.0 - u;
        }
        if self.flip[1] {
            v = 1.0 - v;
        }
        let [cl, ct, cr, cb] = self.crop;
        if u < cl || u > 1.0 - cr || v < ct || v > 1.0 - cb {
            return None;
        }
        let f = self.frame;
        let (fw, fh) = (f.width as i32, f.height as i32);
        let tx = u * f.width as f32 - 0.5;
        let ty = v * f.height as f32 - 0.5;
        let (bx, by) = (tx.floor(), ty.floor());
        let (ax, ay) = (tx - bx, ty - by);
        let (bx, by) = (bx as i32, by as i32);
        let texel = |x: i32, y: i32| -> [f32; 4] {
            let (x, y) = (x.clamp(0, fw - 1) as usize, y.clamp(0, fh - 1) as usize);
            let i = (y * f.width as usize + x) * 4;
            let p = &f.rgba[i..i + 4];
            let a = p[3] as f32 / 255.0;
            [
                p[0] as f32 / 255.0 * a,
                p[1] as f32 / 255.0 * a,
                p[2] as f32 / 255.0 * a,
                a,
            ]
        };
        let (t00, t10) = (texel(bx, by), texel(bx + 1, by));
        let (t01, t11) = (texel(bx, by + 1), texel(bx + 1, by + 1));
        let mut out = [0.0; 4];
        for k in 0..4 {
            let top = t00[k] + (t10[k] - t00[k]) * ax;
            let bottom = t01[k] + (t11[k] - t01[k]) * ax;
            out[k] = (top + (bottom - top) * ay) * self.opacity;
        }
        Some(out)
    }
}

/// Blend premultiplied `src` onto straight `dst` (both 0..1) with `mode`.
fn blend_pixel(dst: &mut [f32], src: [f32; 4], mode: BlendMode) {
    let a_s = src[3];
    if a_s <= 0.0 {
        return;
    }
    let a_b = dst[3];
    let a_o = a_s + a_b * (1.0 - a_s);
    for k in 0..3 {
        let cs = (src[k] / a_s).clamp(0.0, 1.0);
        let cb = dst[k];
        let mixed = blend_channel(mode, cb, cs);
        let co = a_s * (1.0 - a_b) * cs + a_s * a_b * mixed + (1.0 - a_s) * a_b * cb;
        dst[k] = if a_o > 0.0 { co / a_o } else { 0.0 };
    }
    dst[3] = a_o;
}

/// Draw `layers` bottom-up onto a `width` × `height` canvas of
/// `background`; see the module docs. Rows are split across threads.
pub fn composite_cpu(
    width: u32,
    height: u32,
    background: [u8; 4],
    layers: &[CompositeLayer<'_>],
) -> Frame {
    let (w, h) = (width.max(1), height.max(1));
    let bg = background.map(|c| c as f32 / 255.0);
    let mut canvas = vec![0f32; w as usize * h as usize * 4];
    for px in canvas.as_chunks_mut::<4>().0 {
        *px = bg;
    }
    let maps: Vec<Map> = layers.iter().filter_map(|l| Map::new(l, w, h)).collect();
    let row = w as usize * 4;
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(16);
    let rows_per = (h as usize).div_ceil(threads).max(1);
    let draw = |y0: usize, rows: &mut [f32]| {
        for m in &maps {
            let [bx0, by0, bx1, by1] = m.bounds.map(|v| v as usize);
            for (i, line) in rows.chunks_mut(row).enumerate() {
                let y = y0 + i;
                if y < by0 || y >= by1 {
                    continue;
                }
                for x in bx0..bx1 {
                    if let Some(s) = m.sample(x as f32 + 0.5, y as f32 + 0.5) {
                        blend_pixel(&mut line[x * 4..x * 4 + 4], s, m.blend);
                    }
                }
            }
        }
    };
    if maps.is_empty() {
        // Nothing to draw.
    } else if threads == 1 || (w as usize * h as usize) < 65_536 {
        draw(0, &mut canvas);
    } else {
        std::thread::scope(|scope| {
            for (i, rows) in canvas.chunks_mut(rows_per * row).enumerate() {
                let draw = &draw;
                scope.spawn(move || draw(i * rows_per, rows));
            }
        });
    }
    let rgba: Vec<u8> = canvas
        .iter()
        .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect();
    Frame {
        width: w,
        height: h,
        rgba: Arc::from(rgba),
    }
}

/// `frame` with its color multiplied by its alpha.
pub(super) fn premultiplied(frame: &Frame) -> Frame {
    let mut rgba = frame.rgba.to_vec();
    for px in rgba.as_chunks_mut::<4>().0 {
        let a = px[3] as u32;
        if a < 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * a + 127) / 255) as u8;
            }
        }
    }
    Frame {
        rgba: Arc::from(rgba),
        ..*frame
    }
}

/// `frame`'s premultiplied color made straight again.
pub(super) fn unpremultiplied(frame: &Frame) -> Frame {
    let mut rgba = frame.rgba.to_vec();
    for px in rgba.as_chunks_mut::<4>().0 {
        let a = px[3] as u32;
        if a == 0 {
            px[..3].fill(0);
        } else if a < 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    Frame {
        rgba: Arc::from(rgba),
        ..*frame
    }
}

/// `a` → `b` at `progress`, per byte (a cross-dissolve of premultiplied
/// frames of the same size; `b`'s size wins otherwise).
pub(super) fn mix(a: &Frame, b: &Frame, progress: f32) -> Frame {
    if a.width != b.width || a.height != b.height {
        return b.clone();
    }
    let p = progress.clamp(0.0, 1.0);
    let rgba: Vec<u8> = a
        .rgba
        .iter()
        .zip(b.rgba.iter())
        .map(|(x, y)| (*x as f32 + (*y as f32 - *x as f32) * p).round() as u8)
        .collect();
    Frame {
        rgba: Arc::from(rgba),
        ..*b
    }
}

/// A fully transparent frame.
pub(super) fn transparent(width: u32, height: u32) -> Frame {
    Frame {
        width,
        height,
        rgba: Arc::from(vec![0u8; width as usize * height as usize * 4]),
    }
}
