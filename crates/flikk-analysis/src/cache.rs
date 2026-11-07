//! `paths::cache()/analysis/<fingerprint>.json`, with the steps the result
//! covers, and the sound vectors beside it as `<fingerprint>.vectors`,
//! written atomically.

use anyhow::{Context, Result};
use flikk_core::analysis::{ANALYSIS_VERSION, Analysis, AnalysisSteps, SoundVectors};
use flikk_core::model::Fingerprint;
use flikk_core::paths;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A result as written: its fields, and `steps` beside them.
#[derive(Serialize)]
struct Stored<'a> {
    #[serde(flatten)]
    analysis: &'a Analysis,
    steps: &'a AnalysisSteps,
}

/// The steps of a result as read; `None` in one written before they were
/// recorded.
#[derive(Deserialize)]
struct Steps {
    #[serde(default)]
    steps: Option<AnalysisSteps>,
}

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

/// The result and the steps it covers; `None` when missing, unreadable or
/// of another [`ANALYSIS_VERSION`].
pub fn load(fp: &Fingerprint) -> Option<(Analysis, AnalysisSteps)> {
    let bytes = std::fs::read(path(fp)).ok()?;
    let analysis: Analysis = serde_json::from_slice(&bytes).ok()?;
    if analysis.version != ANALYSIS_VERSION {
        return None;
    }
    let steps = serde_json::from_slice::<Steps>(&bytes).ok()?.steps;
    let steps = steps.unwrap_or_else(|| held(&analysis));
    Some((analysis, steps))
}

/// What a result written before steps were recorded covers, as far as
/// what it holds tells: the speech steps with a transcript; shot changes
/// with a transcript or shots, as the sound-only pass made neither; the
/// sound steps always, as every pass ran them, so nothing is heard again
/// that may have been heard. "Analyze again" makes what is missing.
fn held(analysis: &Analysis) -> AnalysisSteps {
    let speech = analysis.transcript.is_some();
    AnalysisSteps {
        transcript: speech,
        speakers: speech,
        shots: speech || !analysis.shot_changes.is_empty(),
        ..AnalysisSteps::default()
    }
}

pub fn store(fp: &Fingerprint, analysis: &Analysis, steps: &AnalysisSteps) -> Result<()> {
    let stored = Stored { analysis, steps };
    write(&path(fp), &serde_json::to_vec(&stored)?)
}

/// `None` when missing or not vectors this version wrote.
#[cfg_attr(not(feature = "sounds"), allow(dead_code))]
pub fn load_vectors(fp: &Fingerprint) -> Option<SoundVectors> {
    SoundVectors::from_bytes(&std::fs::read(file(fp, "vectors")).ok()?)
}

pub fn store_vectors(fp: &Fingerprint, vectors: &SoundVectors) -> Result<()> {
    write(&file(fp, "vectors"), &vectors.to_bytes())
}

/// Remove the vectors of a result that made none.
pub fn remove_vectors(fp: &Fingerprint) -> Result<()> {
    let path = file(fp, "vectors");
    match std::fs::remove_file(&path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(e).with_context(|| format!("could not remove {}", path.display()))
        }
        _ => Ok(()),
    }
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
