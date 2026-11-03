//! Grades on export. A clip's grade and the timeline grade over it are
//! baked through [`flikk_core::grade`] (the code the viewer and scopes use)
//! into one 3D LUT in the render's work dir and applied with `lut3d`, so the
//! render matches the preview; each grade's own LUT file is folded in.

use crate::ffmpeg::escape_path;
use anyhow::{Context as _, Result};
use flikk_core::grade::{self, Cube};
use flikk_core::model::Grade;
use std::hash::{Hash as _, Hasher as _};
use std::path::Path;

/// Grid points per axis. 65³ keeps tetrahedral interpolation within about
/// half an 8-bit step of the exact grade, curves and hue included.
pub const LUT_SIZE: usize = 65;

/// Filters applying `grades` in order; none when they are all neutral.
/// Fails when a grade's LUT file cannot be read.
pub fn filters(grades: &[&Grade], work_dir: &Path) -> Result<Vec<String>> {
    let grades: Vec<&Grade> = grades
        .iter()
        .copied()
        .filter(|g| !grade::is_neutral(g))
        .collect();
    if grades.is_empty() {
        return Ok(Vec::new());
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for g in &grades {
        serde_json::to_string(g).unwrap_or_default().hash(&mut h);
        if let Some(lut) = &g.lut {
            Cube::load(lut).with_context(|| format!("could not use LUT {}", lut.display()))?;
            let modified = std::fs::metadata(lut).and_then(|m| m.modified()).ok();
            modified.hash(&mut h);
        }
    }
    let path = work_dir.join(format!("grade-{:016x}.cube", h.finish()));
    if !path.exists() {
        let mut cube = grade::bake(&grades, LUT_SIZE);
        // `lut3d` truncates to 8 bits; half a step up makes that a rounding.
        for c in &mut cube.data {
            *c = c.map(|v| v + 0.5 / 255.0);
        }
        std::fs::write(&path, cube.to_text("flikk grade"))
            .context("could not write the grade LUT")?;
    }
    // 8-bit RGB in and out, as the viewer grades it.
    Ok(vec![
        "format=rgb24".into(),
        format!("lut3d=file={}:interp=tetrahedral", escape_path(&path)),
    ])
}
