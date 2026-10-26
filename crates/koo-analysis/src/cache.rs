//! `paths::cache()/analysis/<fingerprint>.json`, written atomically.

use anyhow::{Context, Result};
use koo_core::analysis::Analysis;
use koo_core::model::Fingerprint;
use koo_core::paths;
use std::path::PathBuf;

fn path(fp: &Fingerprint) -> PathBuf {
    // Fingerprints are hex; keep anything else out of the file name.
    let name: String =
        fp.0.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
    paths::cache().join("analysis").join(format!("{name}.json"))
}

/// `None` when missing or unreadable (an old format is simply recomputed).
pub fn load(fp: &Fingerprint) -> Option<Analysis> {
    let bytes = std::fs::read(path(fp)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn store(fp: &Fingerprint, analysis: &Analysis) -> Result<()> {
    let dest = path(fp);
    let dir = dest.parent().expect("cache file has a parent");
    std::fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
    let tmp = dest.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec(analysis)?)
        .with_context(|| format!("could not write {}", tmp.display()))?;
    std::fs::rename(&tmp, &dest).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(())
}
