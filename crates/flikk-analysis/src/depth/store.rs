//! Depth maps of single frames kept on disk under
//! `paths::cache()/depth/<fingerprint>/<model>/<frame>.depth`: a small
//! header and the map's distances, a grayscale PNG (they are smooth, so a
//! few times smaller than as bytes; version 1 kept the bytes as they
//! are), written whole and moved into place, so a reader never sees half.
//! Reading one marks it used, so it is not swept
//! (`flikk_core::cache_sweep`).

use anyhow::{Context, Result};
use flikk_core::model::Fingerprint;
use flikk_core::paths;
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 8] = b"FLKDEPTH";
const VERSION: u32 = 2;

/// One frame's map as kept: distance 0 (nearest) to 255, top row first.
#[derive(Clone, Debug)]
pub struct Raw {
    pub width: usize,
    pub height: usize,
    pub values: Vec<u8>,
}

/// `text` fit for a file name: letters, digits, `-` and `.` kept, anything
/// else `_`: model ids have dashes, and dots (`sam2.1-…`).
fn safe(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub fn path(fp: &Fingerprint, model: &str, frame: i64) -> PathBuf {
    paths::cache()
        .join("depth")
        .join(fp.file_stem())
        .join(safe(model))
        .join(format!("{frame}.depth"))
}

/// `None` when missing, unreadable or of another version.
pub fn load(path: &Path) -> Option<Raw> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() < 16 || &bytes[..8] != MAGIC {
        return None;
    }
    let version = u32::from_le_bytes(bytes[8..12].try_into().ok()?);
    let width = u16::from_le_bytes(bytes[12..14].try_into().ok()?) as usize;
    let height = u16::from_le_bytes(bytes[14..16].try_into().ok()?) as usize;
    let values = match version {
        1 => bytes[16..].to_vec(),
        VERSION => decode_png(&bytes[16..], width, height)?,
        _ => return None,
    };
    if width == 0 || height == 0 || values.len() != width * height {
        return None;
    }
    flikk_core::cache_sweep::mark_used(path);
    Some(Raw {
        width,
        height,
        values,
    })
}

fn encode_png(raw: &Raw) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, raw.width as u32, raw.height as u32);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Balanced);
    let mut writer = encoder.write_header().context("depth map")?;
    writer.write_image_data(&raw.values).context("depth map")?;
    writer.finish().context("depth map")?;
    Ok(out)
}

fn decode_png(bytes: &[u8], width: usize, height: usize) -> Option<Vec<u8>> {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().ok()?;
    let mut values = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut values).ok()?;
    let ok = info.color_type == png::ColorType::Grayscale
        && info.bit_depth == png::BitDepth::Eight
        && (info.width as usize, info.height as usize) == (width, height);
    values.truncate(info.buffer_size());
    ok.then_some(values)
}

pub fn store(path: &Path, raw: &Raw) -> Result<()> {
    let png = encode_png(raw)?;
    let mut out = Vec::with_capacity(16 + png.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(raw.width as u16).to_le_bytes());
    out.extend_from_slice(&(raw.height as u16).to_le_bytes());
    out.extend_from_slice(&png);
    flikk_os::fs::write_atomic(path, &out)
        .with_context(|| format!("could not write {}", path.display()))
}
