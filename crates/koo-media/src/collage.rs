//! Collages for the agent's eyes (PRD V1–V3, V6): up to 30 frames in a 6 × 5
//! grid, each stamped with its timeline and source time, cached as PNG.

use crate::decode::still;
use crate::{FfmpegMedia, err, font, proxy, write_atomic};
use koo_core::paths;
use koo_core::services::MediaService;
use koo_core::services::{
    Collage, CollageRequest, Frame, SamplePoint, ServiceError, ServiceResult,
};
use koo_core::time::Rate;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const MAX_TILES: usize = 30;
const COLUMNS: usize = 6;
const GAP: u32 = 4;
const BACKGROUND: [u8; 3] = [24, 24, 24];
/// Bump when the rendering changes, so cached collages are redrawn.
const VERSION: u32 = 1;

pub(crate) fn collage(media: &FfmpegMedia, request: &CollageRequest) -> ServiceResult<Collage> {
    let tiles: Vec<SamplePoint> = request.samples.iter().take(MAX_TILES).cloned().collect();
    if tiles.is_empty() {
        return Err(ServiceError("collage needs at least one sample".into()));
    }
    let tile_width = request.tile_width.clamp(64, 1920) & !1;
    let columns = tiles.len().min(COLUMNS) as u32;
    let rows = tiles.len().div_ceil(COLUMNS) as u32;

    let dir = paths::cache().join("collages");
    let path = dir.join(format!("{}.png", key(&tiles, tile_width)));
    let done = |path: PathBuf| Collage {
        path,
        tiles: tiles.clone(),
        columns,
        rows,
    };
    if path.is_file() {
        return Ok(done(path));
    }

    let frames = decode_all(media, &tiles, tile_width);
    let Some(first) = frames.iter().flatten().next() else {
        let reason = frames.into_iter().find_map(Result::err);
        return Err(reason.unwrap_or_else(|| ServiceError("no frames".into())));
    };
    // Tiles take the shape of the first frame; others are letterboxed.
    let tile_height =
        ((tile_width as u64 * first.height as u64 / first.width.max(1) as u64) as u32).max(2);

    let width = columns * tile_width + (columns + 1) * GAP;
    let height = rows * tile_height + (rows + 1) * GAP;
    let mut canvas = Canvas::new(width, height, BACKGROUND);
    let scale = (tile_width / 160).max(2);
    for (i, (sample, frame)) in tiles.iter().zip(&frames).enumerate() {
        let x = GAP + (i as u32 % columns) * (tile_width + GAP);
        let y = GAP + (i as u32 / columns) * (tile_height + GAP);
        match frame {
            Ok(frame) => canvas.fit(frame, x, y, tile_width, tile_height),
            Err(_) => {
                canvas.fill(x, y, tile_width, tile_height, [48, 16, 16]);
                canvas.label(&["NO FRAME"], x, y, scale, false);
            }
        }
        let timeline = format!("TL {}", sample.timeline);
        let source = format!("SRC {}", sample.source);
        canvas.label(&[&timeline, &source], x, y + tile_height, scale, true);
    }

    std::fs::create_dir_all(&dir).map_err(|e| err("collage dir", e))?;
    let png = canvas.png()?;
    // A collage with missing tiles is returned but not cached, so a later
    // request retries the decode.
    let path = if frames.iter().any(Result::is_err) {
        path.with_extension("partial.png")
    } else {
        path
    };
    write_atomic(&path, &png).map_err(|e| err("collage", e))?;
    Ok(done(path))
}

/// Hash of everything that shows in the image.
fn key(tiles: &[SamplePoint], tile_width: u32) -> String {
    let mut h = blake3::Hasher::new();
    h.update(&VERSION.to_le_bytes());
    h.update(&tile_width.to_le_bytes());
    for t in tiles {
        match &t.fingerprint {
            Some(fp) => h.update(fp.0.as_bytes()),
            None => h.update(t.path.to_string_lossy().as_bytes()),
        };
        h.update(&[0]);
        h.update(&t.source.0.to_le_bytes());
        h.update(&t.timeline.0.to_le_bytes());
    }
    h.finalize().to_hex()[..32].to_string()
}

/// Decode every tile, a few FFmpeg processes at a time.
fn decode_all(
    media: &FfmpegMedia,
    tiles: &[SamplePoint],
    tile_width: u32,
) -> Vec<ServiceResult<Frame>> {
    let workers = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(2, 8);
    let next = std::sync::atomic::AtomicUsize::new(0);
    // One probe per file, for its frame grid.
    let rates = std::sync::Mutex::new(HashMap::new());
    let rate_of = |path: &Path| -> Rate {
        if let Some(rate) = rates.lock().unwrap().get(path) {
            return *rate;
        }
        let rate = media
            .probe(path)
            .ok()
            .and_then(|i| i.video)
            .map(|v| v.rate)
            .unwrap_or_default();
        rates.lock().unwrap().insert(path.to_path_buf(), rate);
        rate
    };
    let mut results: Vec<Option<ServiceResult<Frame>>> = (0..tiles.len()).map(|_| None).collect();
    let slots = std::sync::Mutex::new(&mut results);
    std::thread::scope(|scope| {
        for _ in 0..workers.min(tiles.len()) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(tile) = tiles.get(i) else { break };
                    let path = source_of(tile);
                    let rate = rate_of(&path);
                    let frame = still(media, &path, Some((tile.source, rate)), tile_width);
                    slots.lock().unwrap()[i] = Some(frame);
                }
            });
        }
    });
    results
        .into_iter()
        .map(|r| r.unwrap_or_else(|| Err(ServiceError("not decoded".into()))))
        .collect()
}

/// The proxy when it exists, else the source.
fn source_of(tile: &SamplePoint) -> PathBuf {
    tile.fingerprint
        .as_ref()
        .map(proxy::proxy_path)
        .filter(|p| p.is_file())
        .unwrap_or_else(|| tile.path.clone())
}

struct Canvas {
    width: u32,
    height: u32,
    rgb: Vec<u8>,
}

impl Canvas {
    fn new(width: u32, height: u32, color: [u8; 3]) -> Self {
        let rgb = color.repeat((width * height) as usize);
        Self { width, height, rgb }
    }

    fn put(&mut self, x: u32, y: u32, c: [u8; 3]) {
        if x < self.width && y < self.height {
            let i = ((y * self.width + x) * 3) as usize;
            self.rgb[i..i + 3].copy_from_slice(&c);
        }
    }

    fn fill(&mut self, x: u32, y: u32, w: u32, h: u32, c: [u8; 3]) {
        for yy in y..y + h {
            for xx in x..x + w {
                self.put(xx, yy, c);
            }
        }
    }

    /// Darken a box, so stamps read on any picture.
    fn shade(&mut self, x: u32, y: u32, w: u32, h: u32) {
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
    fn fit(&mut self, frame: &Frame, x: u32, y: u32, w: u32, h: u32) {
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
    fn label(&mut self, lines: &[&str], x: u32, y: u32, scale: u32, bottom: bool) {
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

    fn png(&self) -> ServiceResult<Vec<u8>> {
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
