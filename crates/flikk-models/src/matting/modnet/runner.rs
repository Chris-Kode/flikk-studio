//! The graph run on one frame: resampled to the model's size (short side
//! [`SHORT_SIDE`], both sides a multiple of [`STRIDE`]), scaled to -1..1,
//! channels first; the answer is the people's coverage at that size.

use super::config::{GRAPH, LONG_SIDE_MAX, MEAN, SHORT_SIDE, STD, STRIDE};
use crate::matting::{Alpha, Matter};
use crate::onnx;
use anyhow::{Result, anyhow, bail};
use flikk_core::services::Frame;
use flikk_os::sync::lock;
use ort::session::Session;
use ort::value::Tensor;
use std::sync::Mutex;

/// The graph, loaded on first use. One frame at a time.
#[derive(Default)]
pub struct Modnet {
    graph: Mutex<Option<Session>>,
}

impl Matter for Modnet {
    fn matte(&self, picture: &Frame) -> Result<Alpha> {
        let (w, h) = (picture.width as usize, picture.height as usize);
        if w == 0 || h == 0 || picture.rgba.len() < w * h * 4 {
            bail!("the picture to matte is empty");
        }
        let (mw, mh) = size(w, h);
        let input = Tensor::from_array((vec![1, 3, mh, mw], planes(picture, mw, mh)))
            .map_err(|e| anyhow!("{e}"))?;
        let mut graph = lock(&self.graph);
        // Its files were deleted: let go of it, and fail as if never there.
        if !super::MODEL.is_downloaded() {
            *graph = None;
        }
        if graph.is_none() {
            *graph = Some(onnx::session(&GRAPH.locate()?, GRAPH.what)?);
        }
        let session = graph.as_mut().expect("loaded above");
        let outputs = session
            .run(ort::inputs!["input" => input])
            .map_err(|e| anyhow!("person mask model failed: {e}"))?;
        let (_, values) = outputs["output"]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow!("{e}"))?;
        if values.len() != mw * mh {
            bail!(
                "person mask model gave {} values for {mw} × {mh}",
                values.len()
            );
        }
        Ok(Alpha {
            width: mw,
            height: mh,
            values: values.iter().map(|v| v.clamp(0.0, 1.0)).collect(),
        })
    }
}

/// The model's size for a `w` × `h` picture: the short side at
/// [`SHORT_SIDE`] (the long one at most [`LONG_SIDE_MAX`]), both rounded
/// to the stride.
fn size(w: usize, h: usize) -> (usize, usize) {
    let short = w.min(h) as f32;
    let long = w.max(h) as f32;
    let k = (SHORT_SIDE as f32 / short).min(LONG_SIDE_MAX as f32 / long);
    let side = |v: usize| {
        let s = (v as f32 * k / STRIDE as f32).round() as usize * STRIDE;
        s.max(STRIDE)
    };
    (side(w), side(h))
}

/// `picture` resampled to `w` × `h` (each pixel the mean of the source
/// pixels under it, the nearest one when enlarging), normalized, one plane
/// per channel.
fn planes(picture: &Frame, w: usize, h: usize) -> Vec<f32> {
    let (fw, fh) = (picture.width as usize, picture.height as usize);
    let span = |i: usize, n: usize, of: usize| {
        let a = i * of / n;
        let b = ((i + 1) * of / n).max(a + 1).min(of);
        a..b
    };
    let cols: Vec<_> = (0..w).map(|x| span(x, w, fw)).collect();
    let plane = w * h;
    let mut out = vec![0f32; 3 * plane];
    for y in 0..h {
        let rows = span(y, h, fh);
        for (x, xs) in cols.iter().enumerate() {
            let mut acc = [0f32; 3];
            let mut count = 0f32;
            for sy in rows.clone() {
                let line = &picture.rgba[sy * fw * 4..(sy + 1) * fw * 4];
                for sx in xs.clone() {
                    let p = &line[sx * 4..sx * 4 + 3];
                    acc[0] += p[0] as f32;
                    acc[1] += p[1] as f32;
                    acc[2] += p[2] as f32;
                    count += 1.0;
                }
            }
            for (ch, a) in acc.iter().enumerate() {
                let v = a / count.max(1.0) / 255.0;
                out[ch * plane + y * w + x] = (v - MEAN) / STD;
            }
        }
    }
    out
}
