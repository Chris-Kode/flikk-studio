//! `paths::cache()/analysis/<fingerprint>.json`, and the sound vectors
//! beside it as `<fingerprint>.vectors`, written atomically.

use anyhow::{Context, Result};
use flikk_core::analysis::{ANALYSIS_VERSION, Analysis, SoundVectors};
use flikk_core::model::Fingerprint;
use flikk_core::paths;
use std::path::{Path, PathBuf};

fn path(fp: &Fingerprint) -> PathBuf {
    file(fp, "json")
}

/// Where an item's audio waits while a provider hears it:
/// `paths::cache()/upload/<fingerprint>.flac`.
pub fn upload(fp: &Fingerprint) -> Result<PathBuf> {
    let dir = paths::cache().join("upload");
    std::fs::create_dir_all(&dir).with_context(|| format!("could not create {}", dir.display()))?;
    Ok(dir.join(format!("{}.flac", safe_name(fp))))
}

fn file(fp: &Fingerprint, extension: &str) -> PathBuf {
    paths::cache()
        .join("analysis")
        .join(format!("{}.{extension}", safe_name(fp)))
}

/// Fingerprints are hex; anything else is kept out of file names.
fn safe_name(fp: &Fingerprint) -> String {
    fp.0.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// `None` when missing, unreadable or of another [`ANALYSIS_VERSION`].
pub fn load(fp: &Fingerprint) -> Option<Analysis> {
    let bytes = std::fs::read(path(fp)).ok()?;
    let analysis: Analysis = serde_json::from_slice(&bytes).ok()?;
    (analysis.version == ANALYSIS_VERSION).then_some(analysis)
}

pub fn store(fp: &Fingerprint, analysis: &Analysis) -> Result<()> {
    write(&path(fp), &serde_json::to_vec(analysis)?)
}

/// `None` when missing or not vectors this version wrote.
#[cfg_attr(not(feature = "sounds"), allow(dead_code))]
pub fn load_vectors(fp: &Fingerprint) -> Option<SoundVectors> {
    SoundVectors::from_bytes(&std::fs::read(file(fp, "vectors")).ok()?)
}

#[cfg_attr(not(feature = "sounds"), allow(dead_code))]
pub fn store_vectors(fp: &Fingerprint, vectors: &SoundVectors) -> Result<()> {
    write(&file(fp, "vectors"), &vectors.to_bytes())
}

fn write(dest: &Path, bytes: &[u8]) -> Result<()> {
    let dir = dest.parent().expect("cache file has a parent");
    std::fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
    let mut tmp = dest.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes).with_context(|| format!("could not write {}", tmp.display()))?;
    std::fs::rename(&tmp, dest).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(())
}
