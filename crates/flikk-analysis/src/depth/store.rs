//! Depth maps of single frames kept on disk under
//! `paths::cache()/depth/<fingerprint>/<model>/<frame>.depth`: a small
//! header and the map's distances as bytes, written whole and moved into
//! place, so a reader never sees half.

use anyhow::{Context, Result, bail};
use flikk_core::model::Fingerprint;
use flikk_core::paths;
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 8] = b"FLKDEPTH";
const VERSION: u32 = 1;

/// One frame's map as kept: distance 0 (nearest) to 255, top row first.
#[derive(Clone, Debug)]
pub struct Raw {
    pub width: usize,
    pub height: usize,
    pub values: Vec<u8>,
}

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
        .join(safe(&fp.0))
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
    let values = &bytes[16..];
    if version != VERSION || width == 0 || height == 0 || values.len() != width * height {
        return None;
    }
    Some(Raw {
        width,
        height,
        values: values.to_vec(),
    })
}

pub fn store(path: &Path, raw: &Raw) -> Result<()> {
    let mut out = Vec::with_capacity(16 + raw.values.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(raw.width as u16).to_le_bytes());
    out.extend_from_slice(&(raw.height as u16).to_le_bytes());
    out.extend_from_slice(&raw.values);
    let dir = path.parent().expect("depth file has a parent");
    std::fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, &out).with_context(|| format!("could not write {}", tmp.display()))?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        bail!("could not write {}: {e}", path.display());
    }
    Ok(())
}
