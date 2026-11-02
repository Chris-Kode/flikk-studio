//! Two frames in as one planar RGB tensor (0..1, six channels), the time
//! step as a scalar, the frame between out. The graph pads to a multiple
//! of 32 and crops back itself.

use super::config::{FILE, MAX_WIDTH};
use crate::interpolation::Interpolator;
use crate::onnx::GpuSessions;
use anyhow::{Result, anyhow, bail};
use koo_core::services::Frame;
use ort::value::{Tensor, TensorRef};
use std::sync::{Arc, Mutex};

/// The model, loaded on first use for each size; it makes one frame at a
/// time.
#[derive(Default)]
pub struct Rife {
    sessions: Mutex<GpuSessions>,
}

impl Interpolator for Rife {
    fn between(&self, a: &Frame, b: &Frame, at: f32) -> Result<Frame> {
        if (a.width, a.height) != (b.width, b.height) {
            bail!("frames to interpolate differ in size");
        }
        let (w, h) = (a.width, a.height);
        let (mw, mh) = if w > MAX_WIDTH {
            (
                MAX_WIDTH,
                ((h as u64 * MAX_WIDTH as u64 / w as u64) as u32).max(1),
            )
        } else {
            (w, h)
        };
        let (sa, sb) = (resize(a, mw, mh), resize(b, mw, mh));
        let plane = (mw * mh) as usize;
        let mut input = vec![0f32; plane * 6];
        for (k, frame) in [&sa, &sb].into_iter().enumerate() {
            for (i, px) in frame.rgba.as_chunks::<4>().0.iter().enumerate() {
                for c in 0..3 {
                    input[(k * 3 + c) * plane + i] = px[c] as f32 / 255.0;
                }
            }
        }
        let sizes = [
            ("dynamic_dim_0", 1),
            ("dynamic_dim_1", 6),
            ("dynamic_dim_2", mh as i64),
            ("dynamic_dim_3", mw as i64),
        ];
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let rgba = sessions.run(&FILE.locate()?, FILE.what, &sizes, |session| {
            let shape = [1usize, 6, mh as usize, mw as usize];
            let input =
                TensorRef::from_array_view((shape, &input[..])).map_err(|e| anyhow!("{e}"))?;
            let step =
                Tensor::from_array(((), vec![at.clamp(0.0, 1.0)])).map_err(|e| anyhow!("{e}"))?;
            let outputs = session
                .run(ort::inputs!["input" => input, "timestep" => step])
                .map_err(|e| anyhow!("frame interpolation failed: {e}"))?;
            let (_, out) = outputs["output"]
                .try_extract_tensor::<f32>()
                .map_err(|e| anyhow!("{e}"))?;
            if out.len() < plane * 3 {
                bail!("frame interpolation gave {} values", out.len());
            }
            if out.iter().any(|v| !v.is_finite()) {
                bail!("frame interpolation gave a frame that is not numbers");
            }
            let mut rgba = vec![255u8; plane * 4];
            for i in 0..plane {
                for c in 0..3 {
                    rgba[i * 4 + c] = (out[c * plane + i].clamp(0.0, 1.0) * 255.0).round() as u8;
                }
            }
            Ok(rgba)
        })?;
        let made = Frame {
            width: mw,
            height: mh,
            rgba: Arc::from(rgba),
        };
        Ok(resize(&made, w, h))
    }
}

/// `frame` at `w` × `h`, bilinear; itself when it is that size.
fn resize(frame: &Frame, w: u32, h: u32) -> Frame {
    if (frame.width, frame.height) == (w, h) {
        return frame.clone();
    }
    let (sw, sh) = (frame.width as usize, frame.height as usize);
    let src = &frame.rgba;
    let mut out = vec![0u8; (w * h) as usize * 4];
    let (fx, fy) = (sw as f32 / w as f32, sh as f32 / h as f32);
    for y in 0..h as usize {
        let v = ((y as f32 + 0.5) * fy - 0.5).clamp(0.0, (sh - 1) as f32);
        let (y0, ty) = (v.floor() as usize, v.fract());
        let y1 = (y0 + 1).min(sh - 1);
        for x in 0..w as usize {
            let u = ((x as f32 + 0.5) * fx - 0.5).clamp(0.0, (sw - 1) as f32);
            let (x0, tx) = (u.floor() as usize, u.fract());
            let x1 = (x0 + 1).min(sw - 1);
            for c in 0..4 {
                let p = |xx: usize, yy: usize| src[(yy * sw + xx) * 4 + c] as f32;
                let top = p(x0, y0) * (1.0 - tx) + p(x1, y0) * tx;
                let bottom = p(x0, y1) * (1.0 - tx) + p(x1, y1) * tx;
                out[(y * w as usize + x) * 4 + c] = (top * (1.0 - ty) + bottom * ty).round() as u8;
            }
        }
    }
    Frame {
        width: w,
        height: h,
        rgba: Arc::from(out),
    }
}
