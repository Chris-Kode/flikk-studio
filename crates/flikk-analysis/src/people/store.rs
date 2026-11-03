//! Person mattes packed small, and kept on disk beside the subjects' under
//! `paths::cache()/mattes/<fingerprint>/`, a chunk of frames per file:
//! `a-<model>-<chunk>.alpha`.
//!
//! Coverages are bytes, those within 3 of either end snapped to it (noise
//! on solid background or person), run-length coded: most of a matte is
//! long runs of 0 and 255.

use anyhow::{Context, Result, bail};
use flikk_core::model::Fingerprint;
use flikk_core::paths;
use flikk_core::person::PersonMatte;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAGIC: &[u8; 8] = b"FLKALPHA";
const VERSION: u32 = 1;
const SNAP: u8 = 3;

/// A matte as kept.
#[derive(Clone, Debug)]
pub struct Packed {
    width: u16,
    height: u16,
    runs: Arc<[u8]>,
}

impl Packed {
    pub fn pack(m: &PersonMatte) -> Self {
        let mut runs = Vec::new();
        let mut current: Option<(u8, u32)> = None;
        for a in m.alpha.iter() {
            let q = match *a {
                a if a <= SNAP => 0,
                a if a >= 255 - SNAP => 255,
                a => a,
            };
            current = match current {
                Some((v, n)) if v == q => Some((v, n + 1)),
                Some((v, n)) => {
                    push_run(&mut runs, v, n);
                    Some((q, 1))
                }
                None => Some((q, 1)),
            };
        }
        if let Some((v, n)) = current {
            push_run(&mut runs, v, n);
        }
        Self {
            width: m.width as u16,
            height: m.height as u16,
            runs: runs.into(),
        }
    }

    pub fn unpack(&self) -> Option<PersonMatte> {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut alpha = Vec::with_capacity(w * h);
        let mut i = 0;
        while i < self.runs.len() {
            let v = self.runs[i];
            let (n, used) = read_varint(&self.runs[i + 1..])?;
            i += 1 + used;
            alpha.extend(std::iter::repeat_n(v, n as usize));
        }
        (alpha.len() == w * h).then(|| PersonMatte {
            width: w,
            height: h,
            alpha: alpha.into(),
        })
    }
}

fn push_run(out: &mut Vec<u8>, v: u8, mut n: u32) {
    out.push(v);
    loop {
        let byte = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
}

fn read_varint(bytes: &[u8]) -> Option<(u32, usize)> {
    let mut n = 0u32;
    for (i, b) in bytes.iter().enumerate().take(5) {
        n |= ((b & 0x7f) as u32) << (7 * i);
        if b & 0x80 == 0 {
            return Some((n, i + 1));
        }
    }
    None
}

fn dir(fp: &Fingerprint) -> PathBuf {
    let safe: String =
        fp.0.chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
    paths::cache().join("mattes").join(safe)
}

fn prefix(model: &str) -> String {
    format!("a-{model}-")
}

pub fn chunk_path(fp: &Fingerprint, model: &str, chunk: i64) -> PathBuf {
    dir(fp).join(format!("{}{chunk}.alpha", prefix(model)))
}

/// The chunks of `fp` on `model` kept on disk.
pub fn list(fp: &Fingerprint, model: &str) -> BTreeSet<i64> {
    let prefix = prefix(model);
    let Ok(entries) = std::fs::read_dir(dir(fp)) else {
        return BTreeSet::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            name.strip_prefix(&prefix)?
                .strip_suffix(".alpha")?
                .parse()
                .ok()
        })
        .collect()
}

/// `None` when missing, unreadable or of another version.
pub fn load(path: &Path) -> Option<Vec<Packed>> {
    let bytes = std::fs::read(path).ok()?;
    let mut r = Reader(&bytes);
    if r.take(8)? != MAGIC || r.u32()? != VERSION {
        return None;
    }
    let count = r.u32()?;
    let mut frames = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let width = r.u16()?;
        let height = r.u16()?;
        let len = r.u32()? as usize;
        let runs: Arc<[u8]> = r.take(len)?.into();
        frames.push(Packed {
            width,
            height,
            runs,
        });
    }
    Some(frames)
}

/// Written whole and moved into place, so a reader never sees half.
pub fn store(path: &Path, frames: &[Packed]) -> Result<()> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(frames.len() as u32).to_le_bytes());
    for p in frames {
        out.extend_from_slice(&p.width.to_le_bytes());
        out.extend_from_slice(&p.height.to_le_bytes());
        out.extend_from_slice(&(p.runs.len() as u32).to_le_bytes());
        out.extend_from_slice(&p.runs);
    }
    let dir = path.parent().expect("matte file has a parent");
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

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Some(head)
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
}
