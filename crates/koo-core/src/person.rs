//! Person masks: the people in a picture, cut out of their background by a
//! matting model ([`SubjectService::person`]) with no prompt, frame by
//! frame through the clip.
//!
//! The model's answer for a frame is a [`PersonMatte`]: how much of each
//! pixel is a person, at a reduced size (a 512 short side), soft at hair
//! and motion blur. [`PersonMatte::field`] draws it at a raster's size:
//! upsampled bilinearly, then, by the mask's `edge` amount, eased onto the
//! full-size picture's own edges by a guided filter (He et al.) in the
//! band where the matte is neither clearly in nor out, which brings back
//! the strands of hair the small matte blurs. Expansion grows (or shrinks)
//! the matte by a grey dilation (or erosion), and feather softens it by a
//! box blur run three times, so both keep its soft edges.
//!
//! [`SubjectService::person`]: crate::services::SubjectService::person

use crate::services::Frame;
use crate::subject::{box_mean, gray};
use std::sync::Arc;

/// Coverage below this counts as background for bounds.
const SOLID: u8 = 12;

/// The people in one frame: `width` × `height` coverages over the whole
/// picture (its proportions), row by row, 0 background to 255 person.
#[derive(Clone, Debug)]
pub struct PersonMatte {
    pub width: usize,
    pub height: usize,
    pub alpha: Arc<[u8]>,
}

impl PersonMatte {
    /// The coverage at uv, 0..1, bilinear between pixel centres.
    pub fn sample(&self, u: f32, v: f32) -> f32 {
        let (w, h) = (self.width.max(1), self.height.max(1));
        if self.alpha.len() < w * h {
            return 0.0;
        }
        let x = (u * w as f32 - 0.5).clamp(0.0, (w - 1) as f32);
        let y = (v * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (ax, ay) = (x - x0 as f32, y - y0 as f32);
        let a = |x: usize, y: usize| self.alpha[y * w + x] as f32 / 255.0;
        let top = a(x0, y0) * (1.0 - ax) + a(x1, y0) * ax;
        let bottom = a(x0, y1) * (1.0 - ax) + a(x1, y1) * ax;
        top * (1.0 - ay) + bottom * ay
    }

    /// Whether uv is on a person.
    pub fn contains(&self, uv: [f32; 2]) -> bool {
        self.sample(uv[0], uv[1]) >= 0.5
    }

    /// Fraction of the picture on people.
    pub fn area(&self) -> f32 {
        let sum: u64 = self.alpha.iter().map(|a| *a as u64).sum();
        sum as f32 / 255.0 / self.alpha.len().max(1) as f32
    }

    /// Where the people are in picture uv: left, top, right, bottom.
    pub fn bounds(&self) -> Option<[f32; 4]> {
        let (w, h) = (self.width, self.height);
        let mut b = [usize::MAX, usize::MAX, 0, 0];
        for (i, a) in self.alpha.iter().enumerate().take(w * h) {
            if *a > SOLID {
                let (x, y) = (i % w, i / w);
                b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
            }
        }
        (b[0] != usize::MAX).then(|| {
            [
                b[0] as f32 / w as f32,
                b[1] as f32 / h as f32,
                (b[2] + 1) as f32 / w as f32,
                (b[3] + 1) as f32 / h as f32,
            ]
        })
    }

    /// The coverage on a `w` × `h` raster of the picture: upsampled, its
    /// edge eased onto `picture`'s by `edge` (0..1), grown by `expansion`
    /// and softened by `feather` (fractions of the picture's height).
    pub fn field(
        &self,
        w: u32,
        h: u32,
        edge: f32,
        feather: f32,
        expansion: f32,
        picture: Option<&Frame>,
    ) -> Vec<f32> {
        let (w, h) = (w.max(1) as usize, h.max(1) as usize);
        let mut out = Vec::with_capacity(w * h);
        for y in 0..h {
            let v = (y as f32 + 0.5) / h as f32;
            for x in 0..w {
                out.push(self.sample((x as f32 + 0.5) / w as f32, v));
            }
        }
        let edge = edge.clamp(0.0, 1.0);
        if let Some(p) = picture.filter(|_| edge > 0.0) {
            out = refine(&out, &gray(p, w, h), w, h, edge);
        }
        let grow = expansion * h as f32;
        if grow.abs() >= 0.5 {
            out = morph(&out, w, h, grow.abs().round() as usize, grow > 0.0);
        }
        // Three boxes of radius r spread an edge over about 6r.
        let soft = feather.max(0.0) * h as f32;
        if soft >= 2.0 {
            let r = ((soft / 6.0).round() as usize).max(1);
            for _ in 0..3 {
                out = box_mean(&out, w, h, r);
            }
        }
        out
    }
}

/// `alpha` eased onto the edges of `guide` by a guided filter whose
/// radius grows with `edge`, in the band where the alpha is unsure,
/// blended in by `edge`.
fn refine(alpha: &[f32], guide: &[f32], w: usize, h: usize, edge: f32) -> Vec<f32> {
    // Fine enough for strands, ε small so the picture's edges win.
    let r = ((h as f32 / 270.0 * (1.0 + 3.0 * edge)).round() as usize).max(1);
    let eps = 1e-4 + 1e-3 * (1.0 - edge);
    let mean = |v: &[f32]| box_mean(v, w, h, r);
    let product = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(a, b)| a * b).collect::<Vec<_>>();
    let mean_i = mean(guide);
    let mean_p = mean(alpha);
    let mean_ip = mean(&product(guide, alpha));
    let mean_ii = mean(&product(guide, guide));
    let mut a = vec![0.0; w * h];
    let mut b = vec![0.0; w * h];
    for k in 0..w * h {
        let var = mean_ii[k] - mean_i[k] * mean_i[k];
        let cov = mean_ip[k] - mean_i[k] * mean_p[k];
        a[k] = cov / (var + eps);
        b[k] = mean_p[k] - a[k] * mean_i[k];
    }
    let (mean_a, mean_b) = (mean(&a), mean(&b));
    // Unsure where the neighbourhood is mixed: 0 inside and outside, 1
    // where half of it is person.
    let band = box_mean(alpha, w, h, 2 * r);
    (0..w * h)
        .map(|k| {
            let q = (mean_a[k] * guide[k] + mean_b[k]).clamp(0.0, 1.0);
            let unsure = (4.0 * band[k] * (1.0 - band[k])).clamp(0.0, 1.0);
            let weight = edge * unsure.sqrt();
            alpha[k] + (q - alpha[k]) * weight
        })
        .collect()
}

/// `v` dilated (`grow`) or eroded by a square of radius `r`: the most (or
/// least) over it, in two passes of van Herk and Gil-Werman's running
/// extremes, whatever the radius.
fn morph(v: &[f32], w: usize, h: usize, r: usize, grow: bool) -> Vec<f32> {
    let pick = |a: f32, b: f32| if grow { a.max(b) } else { a.min(b) };
    let pass = |src: &[f32], len: usize, count: usize, stride: usize, step: usize| {
        let mut out = vec![0.0; src.len()];
        let span = 2 * r + 1;
        let fill = if grow { f32::MIN } else { f32::MAX };
        let mut line = vec![fill; len + 2 * r];
        let mut ahead = vec![fill; line.len()];
        let mut behind = vec![fill; line.len()];
        for l in 0..count {
            for i in 0..len {
                line[r + i] = src[l * stride + i * step];
            }
            let n = line.len();
            for start in (0..n).step_by(span) {
                let end = (start + span).min(n);
                ahead[start] = line[start];
                for i in start + 1..end {
                    ahead[i] = pick(ahead[i - 1], line[i]);
                }
                behind[end - 1] = line[end - 1];
                for i in (start..end - 1).rev() {
                    behind[i] = pick(behind[i + 1], line[i]);
                }
            }
            for i in 0..len {
                // The window line[i ..= i + 2r].
                out[l * stride + i * step] = pick(behind[i], ahead[i + 2 * r]);
            }
        }
        out
    };
    let across = pass(v, w, h, w, 1);
    pass(&across, h, w, 1, w)
}
