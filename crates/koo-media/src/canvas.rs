//! An RGB canvas for the pictures made for the agent (collages, audio
//! pictures): frames scaled into boxes, stamps in the 5 × 7 font, PNG out.

use crate::{err, font};
use koo_core::services::{Frame, ServiceResult};

pub(crate) struct Canvas {
    pub width: u32,
    pub height: u32,
    rgb: Vec<u8>,
}

impl Canvas {
    pub(crate) fn new(width: u32, height: u32, color: [u8; 3]) -> Self {
        let rgb = color.repeat((width * height) as usize);
        Self { width, height, rgb }
    }

    pub(crate) fn put(&mut self, x: u32, y: u32, c: [u8; 3]) {
        if x < self.width && y < self.height {
            let i = ((y * self.width + x) * 3) as usize;
            self.rgb[i..i + 3].copy_from_slice(&c);
        }
    }

    pub(crate) fn fill(&mut self, x: u32, y: u32, w: u32, h: u32, c: [u8; 3]) {
        for yy in y..y + h {
            for xx in x..x + w {
                self.put(xx, yy, c);
            }
        }
    }

    /// Darken a box, so stamps read on any picture.
    pub(crate) fn shade(&mut self, x: u32, y: u32, w: u32, h: u32) {
        for yy in y..(y + h).min(self.height) {
            for xx in x..(x + w).min(self.width) {
                let i = ((yy * self.width + xx) * 3) as usize;
                for v in &mut self.rgb[i..i + 3] {
                    *v /= 4;
                }
            }
        }
    }

    /// Draw `frame` centered in the box, scaled to fit (nearest neighbour;
    /// frames are already decoded near tile size).
    pub(crate) fn fit(&mut self, frame: &Frame, x: u32, y: u32, w: u32, h: u32) {
        let (fw, fh) = (frame.width.max(1) as u64, frame.height.max(1) as u64);
        let (dw, dh) = if fw * h as u64 > fh * w as u64 {
            (w, ((fh * w as u64 / fw) as u32).max(1))
        } else {
            (((fw * h as u64 / fh) as u32).max(1), h)
        };
        self.fill(x, y, w, h, [0, 0, 0]);
        let (ox, oy) = (x + (w - dw) / 2, y + (h - dh) / 2);
        for yy in 0..dh {
            let sy = (yy as u64 * fh / dh as u64) as u32;
            for xx in 0..dw {
                let sx = (xx as u64 * fw / dw as u64) as u32;
                let i = ((sy * frame.width + sx) * 4) as usize;
                let p = &frame.rgba[i..i + 3];
                self.put(ox + xx, oy + yy, [p[0], p[1], p[2]]);
            }
        }
    }

    /// Text in a shaded box at the tile's top-left corner, or bottom-left
    /// when `bottom` (then `y` is the tile's bottom edge).
    pub(crate) fn label(&mut self, lines: &[&str], x: u32, y: u32, scale: u32, bottom: bool) {
        let pad = scale * 2;
        let (tw, th) = font::measure(lines);
        let (bw, bh) = (tw * scale + pad * 2, th * scale + pad * 2);
        let by = if bottom { y.saturating_sub(bh) } else { y };
        self.shade(x, by, bw, bh);
        for (row, line) in lines.iter().enumerate() {
            let ly = by + pad + row as u32 * font::LINE * scale;
            for (col, c) in line.chars().enumerate() {
                let lx = x + pad + col as u32 * font::ADVANCE * scale;
                for (gy, bits) in font::glyph(c).iter().enumerate() {
                    for gx in 0..font::WIDTH {
                        if bits & (0x10 >> gx) != 0 {
                            let (px, py) = (lx + gx * scale, ly + gy as u32 * scale);
                            self.fill(px, py, scale, scale, [255, 255, 255]);
                        }
                    }
                }
            }
        }
    }

    /// `text` in `color` without a box, its top-left corner at `x`, `y`.
    pub(crate) fn text(&mut self, text: &str, x: u32, y: u32, scale: u32, color: [u8; 3]) {
        for (col, c) in text.chars().enumerate() {
            let lx = x + col as u32 * font::ADVANCE * scale;
            for (gy, bits) in font::glyph(c).iter().enumerate() {
                for gx in 0..font::WIDTH {
                    if bits & (0x10 >> gx) != 0 {
                        let (px, py) = (lx + gx * scale, y + gy as u32 * scale);
                        self.fill(px, py, scale, scale, color);
                    }
                }
            }
        }
    }

    /// Mix `color` into a box at `alpha` (0..1).
    pub(crate) fn tint(&mut self, x: u32, y: u32, w: u32, h: u32, color: [u8; 3], alpha: f32) {
        for yy in y..(y + h).min(self.height) {
            for xx in x..(x + w).min(self.width) {
                let i = ((yy * self.width + xx) * 3) as usize;
                for (v, c) in self.rgb[i..i + 3].iter_mut().zip(color) {
                    *v = (*v as f32 * (1.0 - alpha) + c as f32 * alpha) as u8;
                }
            }
        }
    }

    pub(crate) fn frame(&self) -> Frame {
        let rgba: Vec<u8> = self
            .rgb
            .chunks(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect();
        Frame {
            width: self.width,
            height: self.height,
            rgba: rgba.into(),
        }
    }

    pub(crate) fn png(&self) -> ServiceResult<Vec<u8>> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, self.width, self.height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header().map_err(|e| err("png", e))?;
        writer
            .write_image_data(&self.rgb)
            .map_err(|e| err("png", e))?;
        writer.finish().map_err(|e| err("png", e))?;
        Ok(out)
    }
}
