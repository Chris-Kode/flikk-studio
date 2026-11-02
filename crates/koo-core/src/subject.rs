//! Subject masks: an object picked in the picture by clicks and a box,
//! which a segmentation model finds ([`SubjectService`]) on the prompted
//! frames and, when the mask tracks it, on every frame its clip shows.
//!
//! Prompts sit on source frames (media time), so trims, splits and speed
//! changes keep them on the frames they were made on; their points are in
//! the picture's uv before flip, like every mask shape. The model's answer
//! for a frame is a [`Matte`]: logits over the whole picture, positive on
//! the object. [`Matte::field`] draws one at a raster's size: upsampled as
//! the model's own postprocessing does, with an edge one pixel soft, eased
//! onto the picture's own edges when the subject refines (a guided filter,
//! a few pixels either side of the edge only), or as signed distances when
//! the mask feathers or expands, so those work as they do for shapes.
//!
//! [`SubjectService`]: crate::services::SubjectService

use crate::services::Frame;
use crate::time::{Rate, Time};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

fn yes() -> bool {
    true
}

/// A click that picks (or, not `include`d, rules out) part of the
/// picture.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubjectPoint {
    /// Picture uv, before flip.
    pub at: [f32; 2],
    #[serde(default = "yes")]
    pub include: bool,
}

/// What picks the object on one frame.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SubjectPrompt {
    /// Source time of the frame.
    pub at: Time,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub points: Vec<SubjectPoint>,
    /// A box around the object: left, top, right, bottom in picture uv.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rect: Option<[f32; 4]>,
}

impl SubjectPrompt {
    pub fn is_empty(&self) -> bool {
        self.points.is_empty() && self.rect.is_none()
    }
}

/// The object a subject mask keeps, as clicked and boxed on some frames.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Subject {
    /// One per frame, sorted by time.
    pub prompts: Vec<SubjectPrompt>,
    /// Followed through every frame the clip shows; otherwise each frame
    /// shows the nearest prompted frame's matte.
    #[serde(default)]
    pub track: bool,
    /// Edges eased onto the picture's own (hair, fur, soft outlines).
    #[serde(default = "yes")]
    pub refine: bool,
}

impl Subject {
    pub fn new(prompt: SubjectPrompt) -> Self {
        Self {
            prompts: vec![prompt],
            track: false,
            refine: true,
        }
    }

    /// The prompt on source frame `frame` at `rate`.
    pub fn prompt_on(&self, frame: i64, rate: Rate) -> Option<&SubjectPrompt> {
        self.prompts.iter().find(|p| p.at.to_frame(rate) == frame)
    }

    /// `self` with `prompt` on its frame at `rate`, in place of the one
    /// there; an empty prompt takes that frame's away.
    pub fn with_prompt(&self, prompt: SubjectPrompt, rate: Rate) -> Subject {
        let frame = prompt.at.to_frame(rate);
        let mut out = self.clone();
        out.prompts.retain(|p| p.at.to_frame(rate) != frame);
        if !prompt.is_empty() {
            out.prompts.push(SubjectPrompt {
                at: Time::from_frame(frame, rate),
                ..prompt
            });
        }
        out.prompts.sort_by_key(|p| p.at);
        out
    }

    /// What the model is asked, as a number that changes with any prompt:
    /// mattes are kept under it.
    pub fn key(&self) -> u64 {
        let mut h = Fnv::default();
        for p in &self.prompts {
            h.write(&p.at.0.to_le_bytes());
            for point in &p.points {
                h.write(&point.at[0].to_bits().to_le_bytes());
                h.write(&point.at[1].to_bits().to_le_bytes());
                h.write(&[point.include as u8]);
            }
            if let Some(r) = p.rect {
                for v in r {
                    h.write(&v.to_bits().to_le_bytes());
                }
            }
            h.write(&[0xff]);
        }
        h.0
    }
}

/// FNV-1a, 64 bits: stable across runs and builds, for cache keys.
pub struct Fnv(pub u64);

impl Default for Fnv {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv {
    pub fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= *b as u64;
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
}

/// A subject on one frame: the model's logits over the whole picture
/// (stretched to a square), positive on the object.
#[derive(Clone, Debug)]
pub struct Matte {
    pub side: usize,
    /// Row by row.
    pub logits: Arc<[f32]>,
    /// How well the model thinks it fits the prompt, 0..1.
    pub score: f32,
    /// Whether the object is in the frame at all.
    pub present: bool,
}

/// How [`Matte::field`] draws.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FieldKind {
    /// Coverage 0..1, the edge one pixel soft.
    Coverage,
    /// Signed distance to the edge in picture heights, negative inside.
    Distance,
}

impl Matte {
    /// The logit at uv, bilinear between cell centres (the model's own
    /// upsampling).
    fn sample(&self, u: f32, v: f32) -> f32 {
        let n = self.side;
        let x = (u * n as f32 - 0.5).clamp(0.0, (n - 1) as f32);
        let y = (v * n as f32 - 0.5).clamp(0.0, (n - 1) as f32);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(n - 1), (y0 + 1).min(n - 1));
        let (ax, ay) = (x - x0 as f32, y - y0 as f32);
        let l = &self.logits;
        let top = l[y0 * n + x0] * (1.0 - ax) + l[y0 * n + x1] * ax;
        let bottom = l[y1 * n + x0] * (1.0 - ax) + l[y1 * n + x1] * ax;
        top * (1.0 - ay) + bottom * ay
    }

    /// The logits at the centres of a `w` × `h` raster.
    fn upsampled(&self, w: usize, h: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(w * h);
        for y in 0..h {
            let v = (y as f32 + 0.5) / h as f32;
            for x in 0..w {
                out.push(self.sample((x as f32 + 0.5) / w as f32, v));
            }
        }
        out
    }

    /// Each raster pixel's signed distance to the edge in pixels (negative
    /// inside), from the upsampled logits over their slope: exact on the
    /// straight-line edge the interpolation draws, saturating far from it.
    fn edge_distance(logits: &[f32], w: usize, h: usize) -> Vec<f32> {
        let at = |x: usize, y: usize| logits[y * w + x];
        let mut out = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let gx = (at((x + 1).min(w - 1), y) - at(x.saturating_sub(1), y))
                    / ((x + 1).min(w - 1) - x.saturating_sub(1)).max(1) as f32;
                let gy = (at(x, (y + 1).min(h - 1)) - at(x, y.saturating_sub(1)))
                    / ((y + 1).min(h - 1) - y.saturating_sub(1)).max(1) as f32;
                let g = gx.hypot(gy).max(1e-6);
                out.push(-at(x, y) / g);
            }
        }
        out
    }

    /// The matte drawn on a `w` × `h` raster of the picture (its
    /// proportions): [`FieldKind::Coverage`], refined onto `picture`'s
    /// edges when given, or [`FieldKind::Distance`].
    pub fn field(&self, w: u32, h: u32, kind: FieldKind, picture: Option<&Frame>) -> Vec<f32> {
        let (w, h) = (w.max(1) as usize, h.max(1) as usize);
        if !self.present {
            return match kind {
                FieldKind::Coverage => vec![0.0; w * h],
                FieldKind::Distance => vec![f32::MAX; w * h],
            };
        }
        let logits = self.upsampled(w, h);
        let near = Self::edge_distance(&logits, w, h);
        match kind {
            FieldKind::Coverage => {
                let coverage: Vec<f32> = near.iter().map(|d| (0.5 - d).clamp(0.0, 1.0)).collect();
                match picture {
                    Some(p) => refine(&coverage, &near, &gray(p, w, h), w, h),
                    None => coverage,
                }
            }
            FieldKind::Distance => {
                let inside: Vec<bool> = near.iter().map(|d| *d <= 0.0).collect();
                let to_inside = distance_transform(&inside, w, h, true);
                let to_outside = distance_transform(&inside, w, h, false);
                let pixel = 1.0 / h as f32;
                near.iter()
                    .zip(inside.iter().zip(to_inside.iter().zip(&to_outside)))
                    .map(|(n, (inside, (din, dout)))| {
                        // Close to the edge the logits place it exactly;
                        // further out, the distance to the other side.
                        let d = if n.abs() < 2.0 {
                            *n
                        } else if *inside {
                            -(dout - 0.5)
                        } else {
                            din - 0.5
                        };
                        d * pixel
                    })
                    .collect()
            }
        }
    }

    /// `self` without specks: islands of fewer than `cells` cells are
    /// dropped and holes as small filled (holes open to the picture's edge
    /// stay), as SAM 2's own video predictor fills small holes.
    pub fn cleaned(mut self, cells: usize) -> Self {
        let n = self.side;
        let mut logits = self.logits.to_vec();
        let mut seen = vec![false; n * n];
        let mut stack = Vec::new();
        let mut region = Vec::new();
        for start in 0..n * n {
            if seen[start] {
                continue;
            }
            let inside = logits[start] > 0.0;
            let mut edge = false;
            region.clear();
            stack.push(start);
            seen[start] = true;
            while let Some(i) = stack.pop() {
                region.push(i);
                let (x, y) = (i % n, i / n);
                edge |= x == 0 || y == 0 || x == n - 1 || y == n - 1;
                let neighbours = [
                    (x > 0).then(|| i - 1),
                    (x + 1 < n).then(|| i + 1),
                    (y > 0).then(|| i - n),
                    (y + 1 < n).then(|| i + n),
                ];
                for j in neighbours.into_iter().flatten() {
                    if !seen[j] && (logits[j] > 0.0) == inside {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
            if region.len() >= cells || (!inside && edge) {
                continue;
            }
            // Just across the edge, so the next pixel out keeps its slope.
            let flip = if inside { -0.5 } else { 0.5 };
            for i in &region {
                logits[*i] = flip;
            }
        }
        self.logits = logits.into();
        self
    }

    /// Fraction of the picture on the object.
    pub fn area(&self) -> f32 {
        if !self.present {
            return 0.0;
        }
        self.logits.iter().filter(|l| **l > 0.0).count() as f32 / self.logits.len().max(1) as f32
    }

    /// The object's bounds in picture uv: left, top, right, bottom.
    pub fn bounds(&self) -> Option<[f32; 4]> {
        if !self.present {
            return None;
        }
        let n = self.side;
        let mut b = [usize::MAX, usize::MAX, 0, 0];
        for (i, l) in self.logits.iter().enumerate() {
            if *l > 0.0 {
                let (x, y) = (i % n, i / n);
                b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
            }
        }
        (b[0] != usize::MAX).then(|| {
            let s = n as f32;
            [
                b[0] as f32 / s,
                b[1] as f32 / s,
                (b[2] + 1) as f32 / s,
                (b[3] + 1) as f32 / s,
            ]
        })
    }

    /// Whether uv is on the object.
    pub fn contains(&self, uv: [f32; 2]) -> bool {
        self.present && self.sample(uv[0], uv[1]) > 0.0
    }

    /// The object's outlines in picture uv, closed, the largest (by area)
    /// first; holes come after the outline they are in. Traced through
    /// the logits (marching squares) and simplified to within a fifth of a
    /// cell.
    pub fn outlines(&self) -> Vec<Vec<[f32; 2]>> {
        if !self.present {
            return Vec::new();
        }
        let n = self.side;
        let s = n as f32;
        let loops = marching_squares(&self.logits, n);
        let mut out: Vec<(f32, Vec<[f32; 2]>)> = loops
            .into_iter()
            .map(|l| {
                let l = simplify(&l, 0.2);
                let uv: Vec<[f32; 2]> = l
                    .iter()
                    .map(|p| [(p[0] / s).clamp(0.0, 1.0), (p[1] / s).clamp(0.0, 1.0)])
                    .collect();
                (polygon_area(&uv).abs(), uv)
            })
            .filter(|(_, l)| l.len() >= 3)
            .collect();
        out.sort_by(|a, b| b.0.total_cmp(&a.0));
        out.into_iter().map(|(_, l)| l).collect()
    }

    /// The largest outline in picture uv as at most `most` points, made
    /// coarser until it fits: a path mask to edit by hand.
    pub fn outline_path(&self, most: usize) -> Option<Vec<[f32; 2]>> {
        if !self.present {
            return None;
        }
        let s = self.side as f32;
        let outline = marching_squares(&self.logits, self.side)
            .into_iter()
            .max_by(|a, b| polygon_area(a).abs().total_cmp(&polygon_area(b).abs()))?;
        let mut tolerance = 0.5;
        let mut points = simplify(&outline, tolerance);
        while points.len() > most.max(3) && tolerance < s {
            tolerance *= 1.5;
            points = simplify(&outline, tolerance);
        }
        (points.len() >= 3).then(|| {
            points
                .iter()
                .map(|p| [(p[0] / s).clamp(0.0, 1.0), (p[1] / s).clamp(0.0, 1.0)])
                .collect()
        })
    }
}

/// `frame`'s luma resampled to `w` × `h`, 0..1, each pixel the mean of
/// the source pixels under it.
pub(crate) fn gray(frame: &Frame, w: usize, h: usize) -> Vec<f32> {
    let (fw, fh) = (frame.width.max(1) as usize, frame.height.max(1) as usize);
    let span = |i: usize, n: usize, of: usize| {
        let a = i * of / n;
        let b = ((i + 1) * of / n).max(a + 1).min(of);
        a..b
    };
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        let ys = span(y, h, fh);
        for x in 0..w {
            let xs = span(x, w, fw);
            let (mut sum, mut count) = (0.0f32, 0.0f32);
            for sy in ys.clone() {
                for sx in xs.clone() {
                    let i = (sy * fw + sx) * 4;
                    if let Some(p) = frame.rgba.get(i..i + 3) {
                        sum += 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32;
                        count += 1.0;
                    }
                }
            }
            out.push(sum / count.max(1.0) / 255.0);
        }
    }
    out
}

/// `coverage` eased onto the edges of `guide` by a guided filter (He et
/// al.), radius a 135th of the height, within three radii of the edge
/// (`near`, signed pixels) and faded out across that band, so the inside
/// and outside stay solid.
fn refine(coverage: &[f32], near: &[f32], guide: &[f32], w: usize, h: usize) -> Vec<f32> {
    const EPS: f32 = 1e-3;
    let r = ((h as f32 / 135.0).round() as usize).max(2);
    let band = 3.0 * r as f32;
    let mean = |v: &[f32]| box_mean(v, w, h, r);
    let product = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(a, b)| a * b).collect::<Vec<_>>();
    let mean_i = mean(guide);
    let mean_p = mean(coverage);
    let mean_ip = mean(&product(guide, coverage));
    let mean_ii = mean(&product(guide, guide));
    let mut a = vec![0.0; w * h];
    let mut b = vec![0.0; w * h];
    for k in 0..w * h {
        let var = mean_ii[k] - mean_i[k] * mean_i[k];
        let cov = mean_ip[k] - mean_i[k] * mean_p[k];
        a[k] = cov / (var + EPS);
        b[k] = mean_p[k] - a[k] * mean_i[k];
    }
    let (mean_a, mean_b) = (mean(&a), mean(&b));
    (0..w * h)
        .map(|k| {
            let q = (mean_a[k] * guide[k] + mean_b[k]).clamp(0.0, 1.0);
            let weight = (1.0 - near[k].abs() / band).clamp(0.0, 1.0);
            coverage[k] * (1.0 - weight) + q * weight
        })
        .collect()
}

/// The mean of `v` over a `2r + 1` square around each pixel, the edges
/// repeated.
pub(crate) fn box_mean(v: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let pass = |src: &[f32], len: usize, count: usize, stride: usize, step: usize| {
        let mut out = vec![0.0; src.len()];
        let at = |line: usize, i: isize| {
            let i = i.clamp(0, len as isize - 1) as usize;
            src[line * stride + i * step]
        };
        let span = (2 * r + 1) as f32;
        for line in 0..count {
            let mut sum: f32 = (-(r as isize)..=r as isize).map(|i| at(line, i)).sum();
            for i in 0..len {
                out[line * stride + i * step] = sum / span;
                sum += at(line, i as isize + r as isize + 1) - at(line, i as isize - r as isize);
            }
        }
        out
    };
    let across = pass(v, w, h, w, 1);
    pass(&across, h, w, 1, w)
}

/// Each pixel's distance in pixels to the nearest pixel that is (when
/// `to_set`) or is not in `set` (Felzenszwalb and Huttenlocher's exact
/// transform); 0 on such pixels.
fn distance_transform(set: &[bool], w: usize, h: usize, to_set: bool) -> Vec<f32> {
    const FAR: f32 = 1e12;
    let mut grid: Vec<f32> = set
        .iter()
        .map(|s| if *s == to_set { 0.0 } else { FAR })
        .collect();
    let mut line = Vec::new();
    for x in 0..w {
        line.clear();
        line.extend((0..h).map(|y| grid[y * w + x]));
        let d = transform_1d(&line);
        for y in 0..h {
            grid[y * w + x] = d[y];
        }
    }
    for y in 0..h {
        let d = transform_1d(&grid[y * w..(y + 1) * w]);
        grid[y * w..(y + 1) * w].copy_from_slice(&d);
    }
    grid.into_iter().map(f32::sqrt).collect()
}

/// The squared distance transform of `f` along a line.
fn transform_1d(f: &[f32]) -> Vec<f32> {
    let n = f.len();
    let mut out = vec![0.0; n];
    if n == 0 {
        return out;
    }
    let mut v = vec![0usize; n];
    let mut z = vec![0f32; n + 1];
    let mut k = 0;
    z[0] = f32::NEG_INFINITY;
    z[1] = f32::INFINITY;
    let meet = |q: usize, p: usize| {
        ((f[q] + (q * q) as f32) - (f[p] + (p * p) as f32)) / (2.0 * q as f32 - 2.0 * p as f32)
    };
    for q in 1..n {
        let mut s = meet(q, v[k]);
        while s <= z[k] {
            k -= 1;
            s = meet(q, v[k]);
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f32::INFINITY;
    }
    k = 0;
    for (q, o) in out.iter_mut().enumerate() {
        while z[k + 1] < q as f32 {
            k += 1;
        }
        let d = q as f32 - v[k] as f32;
        *o = d * d + f[v[k]];
    }
    out
}

/// The closed level-0 lines of an `n` × `n` grid of values at cell
/// centres, in cell units (a value's centre at `i + 0.5`), with the grid
/// surrounded by outside so lines close at its border.
fn marching_squares(values: &[f32], n: usize) -> Vec<Vec<[f32; 2]>> {
    use std::collections::HashMap;
    // Corner (x, y) of the padded grid: value (x - 1, y - 1), outside past
    // the edge.
    let at = |x: usize, y: usize| -> f32 {
        if x == 0 || y == 0 || x > n || y > n {
            -1.0
        } else {
            values[(y - 1) * n + (x - 1)]
        }
    };
    let m = n + 2;
    // Where the line crosses an edge of the padded grid, in cell units.
    let cross = |a: (usize, usize), b: (usize, usize)| -> [f32; 2] {
        let (va, vb) = (at(a.0, a.1), at(b.0, b.1));
        let t = if (va - vb).abs() > 1e-12 {
            (va / (va - vb)).clamp(0.0, 1.0)
        } else {
            0.5
        };
        let p = |c: (usize, usize)| [c.0 as f32 - 0.5, c.1 as f32 - 0.5];
        let (pa, pb) = (p(a), p(b));
        [pa[0] + (pb[0] - pa[0]) * t, pa[1] + (pb[1] - pa[1]) * t]
    };
    // Edge ids: horizontal (x, y)–(x + 1, y) and vertical (x, y)–(x, y + 1).
    let horizontal = |x: usize, y: usize| (y * m + x) * 2;
    let vertical = |x: usize, y: usize| (y * m + x) * 2 + 1;
    let mut segments: Vec<(usize, usize)> = Vec::new();
    let mut points: HashMap<usize, [f32; 2]> = HashMap::new();
    for y in 0..m - 1 {
        for x in 0..m - 1 {
            let corners = [(x, y), (x + 1, y), (x + 1, y + 1), (x, y + 1)];
            let inside = corners.map(|c| at(c.0, c.1) > 0.0);
            let case = inside
                .iter()
                .enumerate()
                .fold(0, |acc, (i, s)| acc | ((*s as usize) << i));
            if case == 0 || case == 15 {
                continue;
            }
            let top = horizontal(x, y);
            let right = vertical(x + 1, y);
            let bottom = horizontal(x, y + 1);
            let left = vertical(x, y);
            let mut edge = |id: usize, a: (usize, usize), b: (usize, usize)| {
                points.entry(id).or_insert_with(|| cross(a, b));
                id
            };
            let t = edge(top, corners[0], corners[1]);
            let r = edge(right, corners[1], corners[2]);
            let b = edge(bottom, corners[3], corners[2]);
            let l = edge(left, corners[0], corners[3]);
            let centre = corners.iter().map(|c| at(c.0, c.1)).sum::<f32>() > 0.0;
            // Each segment runs with the inside on its left as it goes, so
            // outlines turn one way and holes the other.
            let pairs: &[(usize, usize)] = match case {
                1 => &[(l, t)],
                2 => &[(t, r)],
                3 => &[(l, r)],
                4 => &[(r, b)],
                5 if centre => &[(l, b), (r, t)],
                5 => &[(l, t), (r, b)],
                6 => &[(t, b)],
                7 => &[(l, b)],
                8 => &[(b, l)],
                9 => &[(b, t)],
                10 if centre => &[(t, l), (b, r)],
                10 => &[(t, r), (b, l)],
                11 => &[(b, r)],
                12 => &[(r, l)],
                13 => &[(r, t)],
                14 => &[(t, l)],
                _ => &[],
            };
            segments.extend_from_slice(pairs);
        }
    }
    let mut next: HashMap<usize, usize> = HashMap::with_capacity(segments.len());
    for (a, b) in &segments {
        next.insert(*a, *b);
    }
    let mut loops = Vec::new();
    while let Some(&start) = next.keys().next() {
        let mut line = Vec::new();
        let mut at_id = start;
        while let Some(to) = next.remove(&at_id) {
            line.push(points[&at_id]);
            at_id = to;
            if at_id == start {
                break;
            }
        }
        if line.len() >= 3 {
            loops.push(line);
        }
    }
    loops
}

/// A closed polyline with points closer than `tolerance` to the line
/// between their neighbours left out (Douglas-Peucker, split at the two
/// points furthest apart).
fn simplify(points: &[[f32; 2]], tolerance: f32) -> Vec<[f32; 2]> {
    let n = points.len();
    if n <= 4 {
        return points.to_vec();
    }
    let far = (1..n)
        .max_by(|a, b| {
            let d = |i: usize| (points[i][0] - points[0][0]).hypot(points[i][1] - points[0][1]);
            d(*a).total_cmp(&d(*b))
        })
        .unwrap_or(n / 2);
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[far] = true;
    let mut stack = vec![(0, far), (far, n)];
    while let Some((a, b)) = stack.pop() {
        if b - a < 2 {
            continue;
        }
        let (pa, pb) = (points[a], points[b % n]);
        let (dx, dy) = (pb[0] - pa[0], pb[1] - pa[1]);
        let len = dx.hypot(dy).max(1e-9);
        let (mut worst, mut at) = (0.0, a);
        for (i, p) in points.iter().enumerate().take(b).skip(a + 1) {
            let d = ((p[0] - pa[0]) * dy - (p[1] - pa[1]) * dx).abs() / len;
            if d > worst {
                (worst, at) = (d, i);
            }
        }
        if worst > tolerance {
            keep[at] = true;
            stack.push((a, at));
            stack.push((at, b));
        }
    }
    points
        .iter()
        .zip(keep)
        .filter_map(|(p, k)| k.then_some(*p))
        .collect()
}

/// Signed area of a closed polygon (shoelace).
fn polygon_area(points: &[[f32; 2]]) -> f32 {
    let n = points.len();
    (0..n)
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f32>()
        / 2.0
}
