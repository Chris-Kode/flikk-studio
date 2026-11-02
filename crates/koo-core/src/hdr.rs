//! HDR sources and their tone mapping to SDR Rec.709.
//!
//! Phone footage is often HDR: HLG (iPhone, many Android phones, the base
//! layer of Dolby Vision profile 8.4) or PQ (HDR10). Shown as it is, its
//! wide-gamut, high-range signal looks grey and washed out. Probing marks
//! such a video stream with an [`Hdr`]; each item's [`ToneMap`] (Auto by
//! default) says how it is mapped to SDR, and [`Mapping::filters`] gives the
//! FFmpeg filters that do it. The media engine puts them on every decode of
//! the original file (frames, playback, proxies, collages) and the render
//! graph on every segment it opens, so the viewer and the export match.
//!
//! Two ways to do the same mapping, picked at run time from the installed
//! FFmpeg ([`Method::from_filters`]):
//!
//! * [`Method::Zscale`]: `zscale` (zimg) to linear light, BT.2020 → BT.709
//!   primaries, FFmpeg's `tonemap` (Möbius) on the brightest channel, back
//!   to BT.709.
//! * [`Method::Lut`]: the same steps baked here into a 65³ `.cube` over the
//!   non-linear BT.2020 signal ([`Mapping::map`]), applied with `lut3d`,
//!   for FFmpeg builds without zimg.
//!
//! Either way the output is BT.709 (primaries, transfer and matrix, limited
//! range) 10-bit 4:4:4, so what follows treats it like SDR footage.

use crate::grade::Cube;
use crate::model::{MediaItem, VideoStream};
use crate::paths;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// How an HDR signal encodes light.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transfer {
    /// SMPTE ST 2084 (HDR10, Dolby Vision): absolute, up to 10,000 nits.
    Pq,
    /// ARIB STD-B67 (BT.2100 HLG): relative to the display, 1,000 nits here.
    Hlg,
}

impl Transfer {
    /// From ffprobe's `color_transfer`.
    pub fn from_ffprobe(name: &str) -> Option<Self> {
        match name {
            "smpte2084" => Some(Self::Pq),
            "arib-std-b67" => Some(Self::Hlg),
            _ => None,
        }
    }

    /// FFmpeg's and zimg's name.
    pub fn ffmpeg(self) -> &'static str {
        match self {
            Self::Pq => "smpte2084",
            Self::Hlg => "arib-std-b67",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Pq => "PQ",
            Self::Hlg => "HLG",
        }
    }
}

/// What probing found about an HDR video stream.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hdr {
    pub transfer: Transfer,
    /// ffprobe's `color_primaries` (`bt2020`).
    pub primaries: String,
    /// ffprobe's `color_space` (`bt2020nc`).
    pub matrix: String,
    /// `tv` (limited) or `pc` (full).
    pub range: String,
    pub bit_depth: u8,
    /// Brightest the content gets, in nits, from its metadata (MaxCLL, else
    /// the mastering display's peak).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak: Option<f32>,
    /// Dolby Vision profile, when the stream carries a configuration
    /// record; its base layer is what is decoded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dolby_vision: Option<u8>,
}

impl Hdr {
    /// Short description, e.g. "HLG 10-bit" or "PQ 10-bit, 1000 nits".
    pub fn describe(&self) -> String {
        let mut s = format!("{} {}-bit", self.transfer.label(), self.bit_depth);
        if let Some(p) = self.peak {
            s.push_str(&format!(", {p:.0} nits"));
        }
        if let Some(dv) = self.dolby_vision {
            s.push_str(&format!(", Dolby Vision {dv}"));
        }
        s
    }
}

/// The item's HDR video stream, if it has one.
pub fn hdr_of(item: &MediaItem) -> Option<&Hdr> {
    item.info.as_ref()?.video.as_ref()?.hdr.as_deref()
}

/// Whether to tone map an HDR item.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToneMapMode {
    /// Map HDR to SDR Rec.709; nothing for SDR media.
    #[default]
    Auto,
    /// Show the file's signal as it is.
    Off,
}

/// An item's tone mapping (per media item; every clip of it shows the same).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ToneMap {
    #[serde(default)]
    pub mode: ToneMapMode,
    /// Stops of exposure before the curve; 0 puts 100 nits at SDR white.
    #[serde(default)]
    pub exposure: f32,
    /// Brightest the source is taken to get, in nits; `None` uses its
    /// metadata, else [`DEFAULT_PEAK`]. Lower keeps more contrast and clips
    /// highlights above it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak: Option<f32>,
}

impl ToneMap {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// Exposure range offered, in stops.
pub const EXPOSURE: (f32, f32) = (-3.0, 3.0);
/// Peak range offered, in nits.
pub const PEAK: (f32, f32) = (200.0, 10_000.0);
/// Peak for PQ without metadata, and HLG's nominal display peak.
pub const DEFAULT_PEAK: f32 = 1000.0;
/// Nits shown at SDR white with no exposure change.
pub const REFERENCE_WHITE: f32 = 100.0;
/// Möbius' linear part: below this (in SDR white) nothing changes.
pub const MOBIUS_KNEE: f32 = 0.3;
/// Bumped when the mapping changes, so proxies made before are made again.
const VERSION: u32 = 1;

/// How the installed FFmpeg does the mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Method {
    Zscale,
    Lut,
}

impl Method {
    /// From the output of `ffmpeg -filters`: `zscale` and `tonemap` when
    /// both are there, else the baked LUT. `KOOEDITOR_TONEMAP=lut` forces
    /// the LUT.
    pub fn from_filters(listing: &str) -> Self {
        if std::env::var("KOOEDITOR_TONEMAP").is_ok_and(|v| v.eq_ignore_ascii_case("lut")) {
            return Self::Lut;
        }
        let has = |name: &str| {
            listing
                .lines()
                .any(|l| l.split_whitespace().nth(1) == Some(name))
        };
        if has("zscale") && has("tonemap") {
            Self::Zscale
        } else {
            Self::Lut
        }
    }
}

/// The mapping an item's pictures go through: an HDR stream with its tone
/// map on Auto.
#[derive(Clone, Debug, PartialEq)]
pub struct Mapping {
    pub transfer: Transfer,
    pub matrix: String,
    pub range: String,
    /// Source peak, nits.
    pub peak: f32,
    /// Nits that land on linear 1.0 (SDR white) before the curve.
    pub white: f32,
}

impl Mapping {
    /// `None` for SDR media and items whose tone map is Off.
    pub fn of(item: &MediaItem) -> Option<Self> {
        let video = item.info.as_ref()?.video.as_ref()?;
        Self::of_stream(video, &item.tone_map)
    }

    pub fn of_stream(video: &VideoStream, tone_map: &ToneMap) -> Option<Self> {
        let hdr = video.hdr.as_ref()?;
        if tone_map.mode == ToneMapMode::Off {
            return None;
        }
        let source_peak = match hdr.transfer {
            Transfer::Pq => hdr.peak.unwrap_or(DEFAULT_PEAK),
            Transfer::Hlg => DEFAULT_PEAK,
        };
        let peak = tone_map.peak.unwrap_or(source_peak).clamp(PEAK.0, PEAK.1);
        let exposure = tone_map.exposure.clamp(EXPOSURE.0, EXPOSURE.1);
        Some(Self {
            transfer: hdr.transfer,
            matrix: if hdr.matrix.is_empty() || hdr.matrix == "unknown" {
                "bt2020nc".into()
            } else {
                hdr.matrix.clone()
            },
            range: if hdr.range == "pc" { "pc" } else { "tv" }.into(),
            peak: peak.round(),
            white: REFERENCE_WHITE / 2f32.powf(exposure),
        })
    }

    /// Names what the mapped pictures look like, for derived files such as
    /// proxies: `hlg-p1000-w100.0-v1`.
    pub fn key(&self) -> String {
        format!(
            "{}-p{}-w{:.1}-v{VERSION}",
            self.transfer.label().to_lowercase(),
            self.peak,
            self.white
        )
    }

    /// The filters, comma-joined, to put after a decoder (and any scaling)
    /// of the original file. The LUT method writes its table under
    /// `paths::cache()/hdr` the first time.
    pub fn filters(&self, method: Method) -> std::io::Result<String> {
        let tag = "setparams=color_primaries=bt709:color_trc=bt709:colorspace=bt709:range=tv";
        Ok(match method {
            Method::Zscale => format!(
                "zscale=tin={tin}:min={min}:pin=bt2020:rin={rin}:t=linear:npl={white},\
                 format=gbrpf32le,zscale=p=bt709,\
                 tonemap=tonemap=mobius:param={knee}:desat=0:peak={peak},\
                 zscale=t=bt709:m=bt709:r=tv,format=yuv444p10le,{tag}",
                tin = self.transfer.ffmpeg(),
                min = self.matrix,
                rin = self.range,
                white = self.white,
                knee = MOBIUS_KNEE,
                peak = self.peak / self.white,
            ),
            Method::Lut => {
                let lut = self.lut()?;
                let matrix = if self.matrix.starts_with("bt2020") {
                    "bt2020"
                } else {
                    "auto"
                };
                format!(
                    "scale=in_color_matrix={matrix}:in_range={rin},format=gbrp16le,\
                     lut3d=file={file}:interp=tetrahedral,\
                     scale=out_color_matrix=bt709:out_range=tv,format=yuv444p10le,{tag}",
                    rin = self.range,
                    file = filter_path(&lut),
                )
            }
        })
    }

    /// The baked table for [`Method::Lut`], written once per mapping.
    pub fn lut(&self) -> std::io::Result<PathBuf> {
        let dir = paths::cache().join("hdr");
        let path = dir.join(format!("{}.cube", self.key()));
        if path.is_file() {
            return Ok(path);
        }
        std::fs::create_dir_all(&dir)?;
        let cube = Cube::from_fn(LUT_SIZE, |c| self.map(c));
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = dir.join(format!("{}.{}-{n}.tmp", self.key(), std::process::id()));
        std::fs::write(&tmp, cube.to_text("koo-editor tone map"))?;
        std::fs::rename(&tmp, &path).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })?;
        Ok(path)
    }

    /// One non-linear BT.2020 R'G'B' sample (0..1) to BT.709 R'G'B',
    /// step by step as the zscale chain does it.
    pub fn map(&self, rgb: [f32; 3]) -> [f32; 3] {
        let nits = match self.transfer {
            Transfer::Pq => rgb.map(pq_eotf),
            Transfer::Hlg => hlg_display(rgb),
        };
        let linear = nits.map(|n| n / self.white);
        let [r, g, b] = mul(&BT2020_TO_BT709, linear);
        let sig = r.max(g).max(b).max(1e-6);
        let scale = mobius(sig, MOBIUS_KNEE, self.peak / self.white) / sig;
        [r, g, b].map(|v| bt1886_inverse((v * scale).clamp(0.0, 1.0)))
    }
}

/// Grid points per axis of the baked table.
pub const LUT_SIZE: usize = 65;

/// ST 2084 inverse EOTF input (0..1) to nits.
fn pq_eotf(e: f32) -> f32 {
    const M1: f64 = 2610.0 / 16384.0;
    const M2: f64 = 2523.0 / 4096.0 * 128.0;
    const C1: f64 = 3424.0 / 4096.0;
    const C2: f64 = 2413.0 / 4096.0 * 32.0;
    const C3: f64 = 2392.0 / 4096.0 * 32.0;
    let p = (e.clamp(0.0, 1.0) as f64).powf(1.0 / M2);
    let y = ((p - C1).max(0.0) / (C2 - C3 * p)).powf(1.0 / M1);
    (y * 10_000.0) as f32
}

/// HLG signal to display light in nits on a 1,000 nit display: the
/// inverse OETF, then system gamma 1.2 on each channel, as zimg does (BT.2100
/// puts the gamma on luminance; zimg's way is what the zscale method shows).
fn hlg_display(rgb: [f32; 3]) -> [f32; 3] {
    const A: f64 = 0.178_832_77;
    const B: f64 = 0.284_668_92;
    const C: f64 = 0.559_910_73;
    rgb.map(|e| {
        let e = e.clamp(0.0, 1.0) as f64;
        let scene = if e <= 0.5 {
            e * e / 3.0
        } else {
            (((e - C) / A).exp() + B) / 12.0
        };
        (DEFAULT_PEAK as f64 * scene.powf(1.2)) as f32
    })
}

/// FFmpeg `tonemap`'s Möbius curve, `peak` in units of SDR white.
fn mobius(x: f32, j: f32, peak: f32) -> f32 {
    if x <= j {
        return x;
    }
    let a = -j * j * (peak - 1.0) / (j * j - 2.0 * j + peak);
    let b = (j * j - 2.0 * j * peak + peak) / (peak - 1.0).max(1e-6);
    (b * b + 2.0 * b * j + j * j) / (b - a) * (x + a) / (x + b)
}

/// Display light to a BT.709 signal as zimg encodes `t=bt709` from linear
/// display light: BT.1886's inverse, a 1/2.4 power.
fn bt1886_inverse(l: f32) -> f32 {
    l.max(0.0).powf(1.0 / 2.4)
}

/// Linear BT.2020 to linear BT.709 (BT.2087).
const BT2020_TO_BT709: [[f32; 3]; 3] = [
    [1.660_491, -0.587_641, -0.072_850],
    [-0.124_550, 1.132_9, -0.008_349],
    [-0.018_151, -0.100_579, 1.118_73],
];

fn mul(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

/// A path as a filter option value (both escaping levels). Forward slashes
/// work on Windows too.
pub fn filter_path(path: &Path) -> String {
    let value = path.to_string_lossy().replace('\\', "/");
    let mut level1 = String::with_capacity(value.len() + 8);
    for c in value.chars() {
        if matches!(c, '\\' | '\'' | ':') {
            level1.push('\\');
        }
        level1.push(c);
    }
    let mut level2 = String::with_capacity(level1.len() + 8);
    for c in level1.chars() {
        if matches!(c, '\\' | '\'' | '[' | ']' | ',' | ';') {
            level2.push('\\');
        }
        level2.push(c);
    }
    level2
}
