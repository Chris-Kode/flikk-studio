//! Mattes packed small, and kept on disk under
//! `paths::cache()/mattes/<fingerprint>/`: a prompted frame by itself as
//! `p-<key>.matte`, a subject's tracked frames as `t-<key>.mattes`.
//!
//! Logits are kept to a sixteenth, up to ±8 (beyond that a pixel is
//! surely in or out), as bytes run-length coded: most of a matte is one
//! run inside and a few outside.

use anyhow::{Context, Result};
use flikk_core::model::Fingerprint;
use flikk_core::paths;
use flikk_core::subject::Matte;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAGIC: &[u8; 8] = b"FLKMATTE";
const VERSION: u32 = 1;
/// Logit steps per unit.
const SCALE: f32 = 16.0;

/// A matte as kept.
#[derive(Clone, Debug)]
pub struct Packed {
    side: u16,
    score: f32,
    present: bool,
    runs: Arc<[u8]>,
}

impl Packed {
    pub fn pack(m: &Matte) -> Self {
        let mut runs = Vec::new();
        let quantized = m
            .logits
            .iter()
            .map(|l| (l * SCALE).round().clamp(-127.0, 127.0) as i8 as u8);
        let mut current: Option<(u8, u32)> = None;
        for q in quantized {
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
            side: m.side as u16,
            score: m.score,
            present: m.present,
            runs: runs.into(),
        }
    }

    pub fn unpack(&self) -> Option<Matte> {
        let side = self.side as usize;
        let mut logits = Vec::with_capacity(side * side);
        let mut i = 0;
        while i < self.runs.len() {
            let v = self.runs[i] as i8 as f32 / SCALE;
            let (n, used) = read_varint(&self.runs[i + 1..])?;
            i += 1 + used;
            logits.extend(std::iter::repeat_n(v, n as usize));
        }
        (logits.len() == side * side).then(|| Matte {
            side,
            logits: logits.into(),
            score: self.score,
            present: self.present,
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

/// Mattes of frames, and the range of frames a finished tracking covered.
#[derive(Clone, Debug, Default)]
pub struct Frames {
    pub frames: BTreeMap<i64, Packed>,
    pub covered: Option<(i64, i64)>,
}

fn dir(fp: &Fingerprint) -> PathBuf {
    paths::cache().join("mattes").join(fp.file_stem())
}

pub fn prompt_path(fp: &Fingerprint, key: u64) -> PathBuf {
    dir(fp).join(format!("p-{key:016x}.matte"))
}

pub fn track_path(fp: &Fingerprint, key: u64) -> PathBuf {
    dir(fp).join(format!("t-{key:016x}.mattes"))
}

/// `None` when missing, unreadable or of another version.
pub fn load(path: &Path) -> Option<Frames> {
    let bytes = std::fs::read(path).ok()?;
    let mut r = Reader(&bytes);
    if r.take(8)? != MAGIC || r.u32()? != VERSION {
        return None;
    }
    let (lo, hi) = (r.i64()?, r.i64()?);
    let covered = (lo <= hi).then_some((lo, hi));
    let count = r.u32()?;
    let mut frames = BTreeMap::new();
    for _ in 0..count {
        let index = r.i64()?;
        let side = r.u32()? as u16;
        let score = f32::from_bits(r.u32()?);
        let present = r.take(1)?[0] != 0;
        let len = r.u32()? as usize;
        let runs: Arc<[u8]> = r.take(len)?.into();
        frames.insert(
            index,
            Packed {
                side,
                score,
                present,
                runs,
            },
        );
    }
    Some(Frames { frames, covered })
}

/// Written whole and moved into place, so a reader never sees half.
pub fn store(path: &Path, frames: &Frames) -> Result<()> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    let (lo, hi) = frames.covered.unwrap_or((1, 0));
    out.extend_from_slice(&lo.to_le_bytes());
    out.extend_from_slice(&hi.to_le_bytes());
    out.extend_from_slice(&(frames.frames.len() as u32).to_le_bytes());
    for (index, p) in &frames.frames {
        out.extend_from_slice(&index.to_le_bytes());
        out.extend_from_slice(&(p.side as u32).to_le_bytes());
        out.extend_from_slice(&p.score.to_bits().to_le_bytes());
        out.push(p.present as u8);
        out.extend_from_slice(&(p.runs.len() as u32).to_le_bytes());
        out.extend_from_slice(&p.runs);
    }
    flikk_os::fs::write_atomic(path, &out)
        .with_context(|| format!("could not write {}", path.display()))
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

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn i64(&mut self) -> Option<i64> {
        Some(i64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
}
