//! The grade pipeline: one definition of what a [`Grade`] does to a pixel,
//! shared by the viewer, the scopes, shot matching and the export (which
//! bakes it into a 3D LUT, so preview and render agree).
//!
//! Values are display-referred RGB in 0..1. Stages, in order:
//!
//! 1. the grade's LUT file (`.cube`, tetrahedral interpolation);
//! 2. exposure: `× 2^exposure`;
//! 3. white balance: temperature scales red up and blue down, tint scales
//!    green down (magenta) or up, each by up to 25 %;
//! 4. lift, gain, gamma and offset per channel:
//!    `(gain · (v + lift · (1 − v)))^(1/gamma_rgb) + offset`;
//! 5. shadows and highlights: smooth bumps centred on 1/3 and 2/3,
//!    `v + ½ (shadows · 4v(1−v)² + highlights · 4v²(1−v))`;
//! 6. contrast around 0.5;
//! 7. master gamma: `v^(1/gamma)`; the result is clamped to 0..1;
//! 8. hue rotation and saturation as one luma-preserving matrix
//!    (Rec. 709 weights), clamped to 0..1;
//! 9. curves: per channel, then master, each a monotone cubic through its
//!    points.
//!
//! Stages 2–7 and 9 are per channel, so [`Compiled`] tabulates them; only
//! the LUT file and the matrix are evaluated per pixel.

use crate::model::{Curves, Grade};
use crate::paths;
use crate::services::Frame;
use crate::{CoreError, Result};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// Rec. 709 luma weights.
pub const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

/// Samples of the float-indexed tables (linear in between).
const TABLE: usize = 4096;

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

/// Temperature and tint: -1..1 (values beyond that are read as percent).
pub fn unit(v: f32) -> f32 {
    if v.abs() > 1.0 { v / 100.0 } else { v }.clamp(-1.0, 1.0)
}

/// Whether `g` leaves every pixel as it is.
pub fn is_neutral(g: &Grade) -> bool {
    g.lut.is_none() && tone_is_identity(g) && matrix(g).is_none() && g.curves.is_identity()
}

fn tone_is_identity(g: &Grade) -> bool {
    near(g.exposure, 0.0)
        && near(unit(g.temperature), 0.0)
        && near(unit(g.tint), 0.0)
        && g.lift.iter().all(|v| near(*v, 0.0))
        && g.gamma_rgb.iter().all(|v| near(*v, 1.0))
        && g.gain.iter().all(|v| near(*v, 1.0))
        && g.offset.iter().all(|v| near(*v, 0.0))
        && near(g.shadows, 0.0)
        && near(g.highlights, 0.0)
        && near(g.contrast, 1.0)
        && near(g.gamma, 1.0)
}

/// Stages 2–7 for channel `c` (0 = red).
fn tone(g: &Grade, c: usize, v: f32) -> f32 {
    let (temp, tint) = (unit(g.temperature), unit(g.tint));
    let balance = [1.0 + 0.25 * temp, 1.0 - 0.25 * tint, 1.0 - 0.25 * temp][c];
    let mut v = v * 2f32.powf(g.exposure) * balance;
    let lift = g.lift[c].clamp(-1.0, 1.0);
    v = g.gain[c].max(0.0) * (v + lift * (1.0 - v));
    v = pow(v, 1.0 / g.gamma_rgb[c].clamp(0.1, 10.0));
    v += g.offset[c];
    let x = v.clamp(0.0, 1.0);
    let (s, h) = (g.shadows.clamp(-1.0, 1.0), g.highlights.clamp(-1.0, 1.0));
    v += 0.5 * (s * 4.0 * x * (1.0 - x) * (1.0 - x) + h * 4.0 * x * x * (1.0 - x));
    v = (v - 0.5) * g.contrast.clamp(0.0, 10.0) + 0.5;
    v = pow(v, 1.0 / g.gamma.clamp(0.1, 10.0));
    v.clamp(0.0, 1.0)
}

fn pow(v: f32, p: f32) -> f32 {
    if v <= 0.0 { 0.0 } else { v.powf(p) }
}

/// Stage 8 as a matrix (rows produce r, g, b), or `None` when neutral.
fn matrix(g: &Grade) -> Option<[[f32; 3]; 3]> {
    let s = g.saturation.clamp(0.0, 4.0);
    if near(s, 1.0)
        && near(
            g.hue.rem_euclid(360.0).min(360.0 - g.hue.rem_euclid(360.0)),
            0.0,
        )
    {
        return None;
    }
    let (sin, cos) = g.hue.to_radians().sin_cos();
    let [lr, lg, lb] = LUMA;
    // Luma-preserving hue rotation (as CSS `hue-rotate`, with Rec. 709 weights).
    let h = [
        [
            lr + cos * (1.0 - lr) - sin * lr,
            lg - cos * lg - sin * lg,
            lb - cos * lb + sin * (1.0 - lb),
        ],
        [
            lr - cos * lr + sin * 0.143,
            lg + cos * (1.0 - lg) + sin * 0.140,
            lb - cos * lb - sin * 0.283,
        ],
        [
            lr - cos * lr - sin * (1.0 - lr),
            lg - cos * lg + sin * lg,
            lb + cos * (1.0 - lb) + sin * lb,
        ],
    ];
    // Saturation: luma + s · (c − luma).
    let sat: [[f32; 3]; 3] = std::array::from_fn(|r| {
        std::array::from_fn(|c| LUMA[c] * (1.0 - s) + if r == c { s } else { 0.0 })
    });
    Some(std::array::from_fn(|r| {
        std::array::from_fn(|c| (0..3).map(|k| sat[r][k] * h[k][c]).sum())
    }))
}

fn mul(m: &[[f32; 3]; 3], c: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|r| (m[r][0] * c[0] + m[r][1] * c[1] + m[r][2] * c[2]).clamp(0.0, 1.0))
}

// -------------------------------------------------------------------- curves

/// A monotone cubic (Fritsch–Carlson) through sorted control points.
/// Without a point at 0 or 1 the curve passes through (0, 0) or (1, 1);
/// beyond the outer points it is flat.
#[derive(Clone, Debug)]
pub struct Curve {
    x: Vec<f32>,
    y: Vec<f32>,
    m: Vec<f32>,
}

impl Curve {
    /// `None` for an identity curve.
    pub fn new(points: &[[f32; 2]]) -> Option<Self> {
        let full = Self::with_ends(points);
        if full.iter().all(|[x, y]| near(*x, *y)) {
            return None;
        }
        let x: Vec<f32> = full.iter().map(|p| p[0]).collect();
        let y: Vec<f32> = full.iter().map(|p| p[1]).collect();
        let n = x.len();
        let d: Vec<f32> = (0..n - 1)
            .map(|k| (y[k + 1] - y[k]) / (x[k + 1] - x[k]))
            .collect();
        let mut m = vec![0f32; n];
        m[0] = d[0];
        m[n - 1] = d[n - 2];
        for k in 1..n - 1 {
            m[k] = if d[k - 1] * d[k] <= 0.0 {
                0.0
            } else {
                (d[k - 1] + d[k]) / 2.0
            };
        }
        for k in 0..n - 1 {
            if d[k] == 0.0 {
                m[k] = 0.0;
                m[k + 1] = 0.0;
                continue;
            }
            let (a, b) = (m[k] / d[k], m[k + 1] / d[k]);
            let r = a * a + b * b;
            if r > 9.0 {
                let t = 3.0 / r.sqrt();
                m[k] = t * a * d[k];
                m[k + 1] = t * b * d[k];
            }
        }
        Some(Self { x, y, m })
    }

    /// The points sorted, clamped to 0..1, de-duplicated, with the implied
    /// end points added: what the curve actually passes through.
    pub fn with_ends(points: &[[f32; 2]]) -> Vec<[f32; 2]> {
        let mut p: Vec<[f32; 2]> = points
            .iter()
            .filter(|p| p[0].is_finite() && p[1].is_finite())
            .map(|p| [p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0)])
            .collect();
        p.sort_by(|a, b| a[0].total_cmp(&b[0]));
        p.dedup_by(|b, a| (b[0] - a[0]).abs() < 1e-3);
        if p.first().is_none_or(|f| f[0] > 1e-3) {
            p.insert(0, [0.0, 0.0]);
        }
        if p.last().is_none_or(|l| l[0] < 1.0 - 1e-3) {
            p.push([1.0, 1.0]);
        }
        p
    }

    pub fn eval(&self, v: f32) -> f32 {
        let (x, y, m) = (&self.x, &self.y, &self.m);
        let n = x.len();
        if v <= x[0] {
            return y[0];
        }
        if v >= x[n - 1] {
            return y[n - 1];
        }
        let k = x.partition_point(|xi| *xi <= v).clamp(1, n - 1) - 1;
        let h = x[k + 1] - x[k];
        let t = (v - x[k]) / h;
        let (t2, t3) = (t * t, t * t * t);
        let v = (2.0 * t3 - 3.0 * t2 + 1.0) * y[k]
            + (t3 - 2.0 * t2 + t) * h * m[k]
            + (-2.0 * t3 + 3.0 * t2) * y[k + 1]
            + (t3 - t2) * h * m[k + 1];
        v.clamp(0.0, 1.0)
    }
}

/// Per-channel curves followed by the master curve.
#[derive(Clone, Debug, Default)]
struct CurveSet {
    channel: [Option<Curve>; 3],
    master: Option<Curve>,
}

impl CurveSet {
    fn new(c: &Curves) -> Self {
        Self {
            channel: [
                Curve::new(&c.red),
                Curve::new(&c.green),
                Curve::new(&c.blue),
            ],
            master: Curve::new(&c.master),
        }
    }

    fn eval(&self, c: usize, v: f32) -> f32 {
        let v = self.channel[c].as_ref().map_or(v, |k| k.eval(v));
        self.master.as_ref().map_or(v, |k| k.eval(v))
    }

    fn is_identity(&self) -> bool {
        self.master.is_none() && self.channel.iter().all(Option::is_none)
    }
}

// ------------------------------------------------------------------ compiled

/// A grade prepared for many pixels: the LUT file loaded, the per-channel
/// stages tabulated.
pub struct Compiled {
    grade: Grade,
    lut: Option<Arc<Cube>>,
    curves: CurveSet,
    matrix: Option<[[f32; 3]; 3]>,
    /// Tone stage for 8-bit input (no LUT file), exact.
    tone8: Option<[[f32; 256]; 3]>,
    /// Tone stage for float input, `TABLE + 1` samples per channel.
    tone: Vec<[f32; 3]>,
    /// Curves to 8-bit output, `TABLE + 1` samples per channel.
    post: Vec<[u8; 3]>,
    /// Whole grade for 8-bit input when it is per channel (no LUT file, no
    /// matrix).
    direct: Option<[[u8; 256]; 3]>,
    neutral: bool,
}

impl Compiled {
    /// Prepare `g`. A LUT file that cannot be read is skipped (the export
    /// reports it).
    pub fn new(g: &Grade) -> Self {
        let lut = g.lut.as_deref().and_then(|p| Cube::load(p).ok());
        let curves = CurveSet::new(&g.curves);
        let matrix = matrix(g);
        let identity_tone = tone_is_identity(g);
        let table: Vec<[f32; 3]> = (0..=TABLE)
            .map(|i| std::array::from_fn(|c| tone(g, c, i as f32 / TABLE as f32)))
            .collect();
        let post: Vec<[u8; 3]> = (0..=TABLE)
            .map(|i| {
                std::array::from_fn(|c| {
                    (curves.eval(c, i as f32 / TABLE as f32) * 255.0).round() as u8
                })
            })
            .collect();
        let tone8 = lut.is_none().then(|| {
            std::array::from_fn(|c| std::array::from_fn(|i| tone(g, c, i as f32 / 255.0)))
        });
        let direct = match (&tone8, &matrix) {
            (Some(t), None) => Some(std::array::from_fn(|c| {
                std::array::from_fn(|i| {
                    let v = curves.eval(c, t[c][i]);
                    (v * 255.0).round() as u8
                })
            })),
            _ => None,
        };
        let neutral = lut.is_none() && identity_tone && matrix.is_none() && curves.is_identity();
        Self {
            grade: g.clone(),
            lut,
            curves,
            matrix,
            tone8,
            tone: table,
            post,
            direct,
            neutral,
        }
    }

    pub fn grade(&self) -> &Grade {
        &self.grade
    }

    pub fn is_neutral(&self) -> bool {
        self.neutral
    }

    /// One color through the whole grade, evaluated exactly (no tables).
    pub fn eval(&self, c: [f32; 3]) -> [f32; 3] {
        let c = match &self.lut {
            Some(l) => l.sample(c),
            None => c.map(|v| v.clamp(0.0, 1.0)),
        };
        let c: [f32; 3] = std::array::from_fn(|i| tone(&self.grade, i, c[i]));
        let c = match &self.matrix {
            Some(m) => mul(m, c),
            None => c,
        };
        std::array::from_fn(|i| self.curves.eval(i, c[i]))
    }

    /// Grade RGBA pixels in place; alpha is kept. Large buffers are split
    /// across threads.
    pub fn apply(&self, rgba: &mut [u8]) {
        if self.neutral {
            return;
        }
        const CHUNK: usize = 1 << 18;
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get().min(8));
        if threads < 2 || rgba.len() < 2 * CHUNK {
            return self.apply_serial(rgba);
        }
        let per = rgba.len().div_ceil(threads * 4) * 4;
        std::thread::scope(|s| {
            for part in rgba.chunks_mut(per) {
                s.spawn(move || self.apply_serial(part));
            }
        });
    }

    fn apply_serial(&self, rgba: &mut [u8]) {
        let px = rgba.as_chunks_mut::<4>().0;
        if let Some(d) = &self.direct {
            for p in px {
                p[0] = d[0][p[0] as usize];
                p[1] = d[1][p[1] as usize];
                p[2] = d[2][p[2] as usize];
            }
            return;
        }
        let post = &self.post;
        let out = |v: f32, c: usize| post[(v * TABLE as f32 + 0.5) as usize][c];
        for p in px {
            let c = match (&self.tone8, &self.lut) {
                (Some(t), _) => [
                    t[0][p[0] as usize],
                    t[1][p[1] as usize],
                    t[2][p[2] as usize],
                ],
                (None, Some(l)) => {
                    let c = l.sample([p[0], p[1], p[2]].map(|v| v as f32 / 255.0));
                    let t = &self.tone;
                    [lerp(t, 0, c[0]), lerp(t, 1, c[1]), lerp(t, 2, c[2])]
                }
                (None, None) => unreachable!("tone8 exists without a LUT file"),
            };
            let c = match &self.matrix {
                Some(m) => mul(m, c),
                None => c,
            };
            p[0] = out(c[0], 0);
            p[1] = out(c[1], 1);
            p[2] = out(c[2], 2);
        }
    }
}

/// Table lookup with linear interpolation, `v` in 0..1.
fn lerp(t: &[[f32; 3]], c: usize, v: f32) -> f32 {
    let x = v.clamp(0.0, 1.0) * TABLE as f32;
    let i = (x as usize).min(TABLE - 1);
    let f = x - i as f32;
    t[i][c] + (t[i + 1][c] - t[i][c]) * f
}

/// The compiled form of `g`, shared with recent callers (playback grades
/// the same few grades every frame).
pub fn compiled(g: &Grade) -> Arc<Compiled> {
    static RECENT: Mutex<Vec<Arc<Compiled>>> = Mutex::new(Vec::new());
    let mut recent = RECENT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(i) = recent.iter().position(|c| c.grade == *g) {
        let c = recent.remove(i);
        recent.push(c.clone());
        return c;
    }
    let c = Arc::new(Compiled::new(g));
    recent.push(c.clone());
    if recent.len() > 8 {
        recent.remove(0);
    }
    c
}

/// Grade RGBA pixels in place.
pub fn apply(rgba: &mut [u8], g: &Grade) {
    if is_neutral(g) {
        return;
    }
    compiled(g).apply(rgba);
}

/// A copy of `frame` graded by `grades` in order (clip, then timeline).
pub fn graded(frame: &Frame, grades: &[&Grade]) -> Frame {
    let mut rgba = frame.rgba.to_vec();
    for g in grades {
        apply(&mut rgba, g);
    }
    Frame {
        width: frame.width,
        height: frame.height,
        rgba: rgba.into(),
    }
}

/// One color through `grades` in order (clip grade, then timeline grade).
pub fn eval(grades: &[&Grade], c: [f32; 3]) -> [f32; 3] {
    grades.iter().fold(c, |c, g| compiled(g).eval(c))
}

// --------------------------------------------------------------------- cube

/// A 3D LUT in the `.cube` format (Adobe/Resolve): red varies fastest.
#[derive(Clone, Debug, PartialEq)]
pub struct Cube {
    pub size: usize,
    pub domain_min: [f32; 3],
    pub domain_max: [f32; 3],
    pub data: Vec<[f32; 3]>,
}

impl Cube {
    pub fn identity(size: usize) -> Self {
        Self::from_fn(size, |c| c)
    }

    /// Sample `f` on a `size`³ grid over 0..1.
    pub fn from_fn(size: usize, f: impl Fn([f32; 3]) -> [f32; 3]) -> Self {
        let size = size.clamp(2, 256);
        let s = (size - 1) as f32;
        let mut data = Vec::with_capacity(size * size * size);
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    data.push(f([r as f32 / s, g as f32 / s, b as f32 / s]));
                }
            }
        }
        Self {
            size,
            domain_min: [0.0; 3],
            domain_max: [1.0; 3],
            data,
        }
    }

    pub fn parse(text: &str) -> Result<Self> {
        let bad = |m: String| CoreError::Invalid(format!("LUT: {m}"));
        let mut size = 0usize;
        let (mut domain_min, mut domain_max) = ([0f32; 3], [1f32; 3]);
        let mut data = Vec::new();
        let triple = |rest: &[&str]| -> Option<[f32; 3]> {
            if rest.len() < 3 {
                return None;
            }
            let v: Vec<f32> = rest[..3].iter().filter_map(|s| s.parse().ok()).collect();
            (v.len() == 3).then(|| [v[0], v[1], v[2]])
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let words: Vec<&str> = line.split_whitespace().collect();
            match words[0] {
                "TITLE" | "LUT_3D_INPUT_RANGE" => {}
                "LUT_1D_SIZE" => return Err(bad("1D LUTs are not supported".into())),
                "LUT_3D_SIZE" => {
                    size = words
                        .get(1)
                        .and_then(|s| s.parse().ok())
                        .filter(|s| (2..=256).contains(s))
                        .ok_or_else(|| bad("bad LUT_3D_SIZE".into()))?;
                    data.reserve(size * size * size);
                }
                "DOMAIN_MIN" => {
                    domain_min = triple(&words[1..]).ok_or_else(|| bad("bad DOMAIN_MIN".into()))?
                }
                "DOMAIN_MAX" => {
                    domain_max = triple(&words[1..]).ok_or_else(|| bad("bad DOMAIN_MAX".into()))?
                }
                w if w.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '.') => {
                    data.push(triple(&words).ok_or_else(|| bad(format!("bad line \"{line}\"")))?);
                }
                _ => {}
            }
        }
        if size == 0 {
            return Err(bad("no LUT_3D_SIZE".into()));
        }
        if data.len() != size * size * size {
            return Err(bad(format!(
                "{} entries, expected {}",
                data.len(),
                size * size * size
            )));
        }
        Ok(Self {
            size,
            domain_min,
            domain_max,
            data,
        })
    }

    /// Load `path`, cached until the file changes.
    pub fn load(path: &Path) -> Result<Arc<Self>> {
        type Entry = (PathBuf, Option<SystemTime>, Arc<Cube>);
        static CACHE: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
        let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        {
            let cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((_, _, c)) = cache.iter().find(|(p, m, _)| p == path && *m == modified) {
                return Ok(c.clone());
            }
        }
        let text = std::fs::read_to_string(path).map_err(|e| {
            CoreError::Invalid(format!("could not read LUT {}: {e}", path.display()))
        })?;
        let cube = Arc::new(Self::parse(&text)?);
        let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        cache.retain(|(p, _, _)| p != path);
        cache.push((path.to_path_buf(), modified, cube.clone()));
        if cache.len() > 16 {
            cache.remove(0);
        }
        Ok(cube)
    }

    /// `.cube` text, six decimals.
    pub fn to_text(&self, title: &str) -> String {
        use std::fmt::Write as _;
        let mut s = String::with_capacity(self.data.len() * 28 + 128);
        let _ = writeln!(s, "TITLE \"{title}\"");
        let _ = writeln!(s, "LUT_3D_SIZE {}", self.size);
        let [a, b, c] = self.domain_min;
        let _ = writeln!(s, "DOMAIN_MIN {a} {b} {c}");
        let [a, b, c] = self.domain_max;
        let _ = writeln!(s, "DOMAIN_MAX {a} {b} {c}");
        for [r, g, b] in &self.data {
            let _ = writeln!(s, "{r:.6} {g:.6} {b:.6}");
        }
        s
    }

    /// Tetrahedral interpolation, as ffmpeg's `lut3d` does by default.
    pub fn sample(&self, c: [f32; 3]) -> [f32; 3] {
        let n = self.size;
        let s = (n - 1) as f32;
        let p: [f32; 3] = std::array::from_fn(|i| {
            let span = (self.domain_max[i] - self.domain_min[i]).max(1e-6);
            (((c[i] - self.domain_min[i]) / span).clamp(0.0, 1.0)) * s
        });
        let i0: [usize; 3] = std::array::from_fn(|i| (p[i] as usize).min(n - 2));
        let [fr, fg, fb] = std::array::from_fn::<f32, 3, _>(|i| p[i] - i0[i] as f32);
        let at = |dr: usize, dg: usize, db: usize| {
            self.data[(i0[0] + dr) + (i0[1] + dg) * n + (i0[2] + db) * n * n]
        };
        let c000 = at(0, 0, 0);
        let c111 = at(1, 1, 1);
        let (w, a, b) = if fr > fg {
            if fg > fb {
                ([1.0 - fr, fr - fg, fg - fb, fb], at(1, 0, 0), at(1, 1, 0))
            } else if fr > fb {
                ([1.0 - fr, fr - fb, fb - fg, fg], at(1, 0, 0), at(1, 0, 1))
            } else {
                ([1.0 - fb, fb - fr, fr - fg, fg], at(0, 0, 1), at(1, 0, 1))
            }
        } else if fb > fg {
            ([1.0 - fb, fb - fg, fg - fr, fr], at(0, 0, 1), at(0, 1, 1))
        } else if fb > fr {
            ([1.0 - fg, fg - fb, fb - fr, fr], at(0, 1, 0), at(0, 1, 1))
        } else {
            ([1.0 - fg, fg - fr, fr - fb, fb], at(0, 1, 0), at(1, 1, 0))
        };
        std::array::from_fn(|i| w[0] * c000[i] + w[1] * a[i] + w[2] * b[i] + w[3] * c111[i])
    }
}

/// `grades` in order (clip, then timeline) baked into one `size`³ LUT,
/// each grade's own LUT file included.
pub fn bake(grades: &[&Grade], size: usize) -> Cube {
    let compiled: Vec<Arc<Compiled>> = grades.iter().map(|g| compiled(g)).collect();
    Cube::from_fn(size, |c| compiled.iter().fold(c, |c, g| g.eval(c)))
}

/// Where the LUT dropdown looks: `<home>/luts`.
pub fn luts_dir() -> PathBuf {
    paths::home().join("luts")
}

/// `.cube` files in [`luts_dir`], by name.
pub fn list_luts() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(luts_dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("cube"))
        })
        .collect();
    out.sort_by_key(|p| p.file_name().map(|n| n.to_ascii_lowercase()));
    out
}
