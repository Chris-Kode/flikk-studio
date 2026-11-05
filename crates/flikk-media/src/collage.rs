//! Collages for the agent's eyes (PRD V1–V3, V6): up to 30 frames in a 6 × 5
//! grid, each stamped with its timeline and source time, cached as PNG.

use crate::canvas::Canvas;
use crate::decode::still;
use crate::{FfmpegMedia, err, proxy, write_atomic};
use flikk_core::hdr::{Mapping, ToneMap};
use flikk_core::paths;
use flikk_core::services::MediaService;
use flikk_core::services::{
    Collage, CollageRequest, Frame, SamplePoint, ServiceError, ServiceResult,
};
use flikk_core::time::Rate;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const MAX_TILES: usize = 30;
const COLUMNS: usize = 6;
const GAP: u32 = 4;
const BACKGROUND: [u8; 3] = [24, 24, 24];
/// Bump when the rendering changes, so cached collages are redrawn.
const VERSION: u32 = 2;

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
        flikk_core::cache_sweep::mark_used(&path);
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
    // One probe per file, for its frame grid and whether it is HDR (shown
    // with the default tone map).
    let probed = std::sync::Mutex::new(HashMap::new());
    let probe_of = |path: &Path| -> (Rate, Option<Mapping>) {
        if let Some(found) = probed.lock().unwrap().get(path) {
            return Clone::clone(found);
        }
        let video = media.probe(path).ok().and_then(|i| i.video);
        let found = (
            video.as_ref().map(|v| v.rate).unwrap_or_default(),
            video
                .as_ref()
                .and_then(|v| Mapping::of_stream(v, &ToneMap::default())),
        );
        probed
            .lock()
            .unwrap()
            .insert(path.to_path_buf(), found.clone());
        found
    };
    let mut results: Vec<Option<ServiceResult<Frame>>> = (0..tiles.len()).map(|_| None).collect();
    let slots = std::sync::Mutex::new(&mut results);
    std::thread::scope(|scope| {
        for _ in 0..workers.min(tiles.len()) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(tile) = tiles.get(i) else { break };
                    let (rate, mapping) = probe_of(&tile.path);
                    let frame =
                        source_of(media, tile, mapping.as_ref()).and_then(|(path, tone)| {
                            let at = Some((tile.source, rate));
                            still(media, &path, at, tile_width, tone.as_deref())
                        });
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

/// The proxy when it exists, else the source with its tone mapping.
fn source_of(
    media: &FfmpegMedia,
    tile: &SamplePoint,
    mapping: Option<&Mapping>,
) -> ServiceResult<(PathBuf, Option<String>)> {
    let proxy = tile
        .fingerprint
        .as_ref()
        .map(|fp| proxy::proxy_path_mapped(fp, mapping))
        .filter(|p| p.is_file());
    match proxy {
        Some(proxy) => Ok((proxy, None)),
        None => Ok((tile.path.clone(), media.tone_filters(mapping)?)),
    }
}
