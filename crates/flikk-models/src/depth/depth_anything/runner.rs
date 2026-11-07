//! The graph run on one picture: scaled (keeping its shape) to
//! [`INPUT`] on the longer side, both sides a multiple of [`PATCH`],
//! normalized; out comes disparity at that size.

use super::config::{INPUT, MEAN, MODEL_FILE, PATCH, STD};
use crate::depth::{DepthEstimator, Disparity};
use crate::onnx::GpuSessions;
use anyhow::{Result, anyhow, bail};
use flikk_core::services::Frame;
use ort::value::TensorRef;
use std::sync::Mutex;

/// The graph, loaded on first use for each size. One picture at a time.
pub struct DepthAnything {
    sessions: Mutex<GpuSessions>,
}

impl Default for DepthAnything {
    fn default() -> Self {
        Self {
            sessions: Mutex::new(GpuSessions::keeping_compiled()),
        }
    }
}

/// The size the model sees a `width` × `height` picture at.
pub fn input_size(width: u32, height: u32) -> (usize, usize) {
    let (w, h) = (width.max(1) as f64, height.max(1) as f64);
    let k = INPUT as f64 / w.max(h);
    let side = |v: f64| (((v * k) / PATCH as f64).round() as usize).max(1) * PATCH;
    (side(w), side(h))
}

/// `picture` resampled bilinearly to `w` × `h` and normalized, planar RGB.
fn pixels(picture: &Frame, w: usize, h: usize) -> Result<Vec<f32>> {
    let (pw, ph) = (picture.width as usize, picture.height as usize);
    if pw == 0 || ph == 0 || picture.rgba.len() < pw * ph * 4 {
        bail!("the picture is empty");
    }
    let mut out = vec![0f32; 3 * w * h];
    let at = |x: usize, y: usize, c: usize| picture.rgba[(y * pw + x) * 4 + c] as f32 / 255.0;
    for y in 0..h {
        let sy = ((y as f32 + 0.5) * ph as f32 / h as f32 - 0.5).clamp(0.0, (ph - 1) as f32);
        let (y0, fy) = (sy.floor() as usize, sy.fract());
        let y1 = (y0 + 1).min(ph - 1);
        for x in 0..w {
            let sx = ((x as f32 + 0.5) * pw as f32 / w as f32 - 0.5).clamp(0.0, (pw - 1) as f32);
            let (x0, fx) = (sx.floor() as usize, sx.fract());
            let x1 = (x0 + 1).min(pw - 1);
            for c in 0..3 {
                let top = at(x0, y0, c) * (1.0 - fx) + at(x1, y0, c) * fx;
                let bottom = at(x0, y1, c) * (1.0 - fx) + at(x1, y1, c) * fx;
                let v = top * (1.0 - fy) + bottom * fy;
                out[c * w * h + y * w + x] = (v - MEAN[c]) / STD[c];
            }
        }
    }
    Ok(out)
}

impl DepthEstimator for DepthAnything {
    fn estimate(&self, picture: &Frame) -> Result<Disparity> {
        let (w, h) = input_size(picture.width, picture.height);
        let input = pixels(picture, w, h)?;
        let sizes = [("batch_size", 1), ("height", h as i64), ("width", w as i64)];
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        // Its file was deleted: let go of it, and fail as if never there.
        if !super::MODEL.is_downloaded() {
            sessions.forget();
        }
        sessions.run(&MODEL_FILE.locate()?, MODEL_FILE.what, &sizes, |session| {
            let input = TensorRef::from_array_view(([1, 3, h, w], &input[..]))
                .map_err(|e| anyhow!("{e}"))?;
            let outputs = session
                .run(ort::inputs!["pixel_values" => input])
                .map_err(|e| anyhow!("depth model failed: {e}"))?;
            let (shape, values) = outputs["predicted_depth"]
                .try_extract_tensor::<f32>()
                .map_err(|e| anyhow!("{e}"))?;
            let dims: Vec<usize> = shape.iter().map(|d| *d as usize).collect();
            let (oh, ow) = match dims.as_slice() {
                [_, oh, ow] | [_, _, oh, ow] => (*oh, *ow),
                _ => bail!("depth model gave a map of unexpected shape {dims:?}"),
            };
            if values.len() != ow * oh {
                bail!("depth model gave a map of unexpected size");
            }
            if values.iter().any(|v| !v.is_finite()) {
                bail!("depth model gave a map that is not numbers");
            }
            Ok(Disparity {
                width: ow,
                height: oh,
                values: values.to_vec(),
            })
        })
    }
}
