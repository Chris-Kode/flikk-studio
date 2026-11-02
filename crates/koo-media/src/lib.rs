//! Media engine on FFmpeg: probe, fingerprint, proxies, frame decode,
//! collages, waveforms and audio pictures. Implements `koo_core::services::MediaService`.
//!
//! FFmpeg is used as a pair of binaries (`KOOEDITOR_FFMPEG` /
//! `KOOEDITOR_FFPROBE`, else `PATH`), never linked. Every call blocks; callers
//! run them off the UI thread. Derived files live under `paths::cache()`,
//! keyed by fingerprint, and sources are only ever read.

mod canvas;
mod collage;
mod decode;
mod fingerprint;
mod font;
mod probe;
mod proxy;
mod spectrogram;
mod waveform;

pub use probe::media_kind;
pub use proxy::{proxy_path, proxy_path_for};

use decode::FrameCache;
use koo_core::hdr;
use koo_core::model::{Fingerprint, MediaInfo, MediaItem};
use koo_core::services::{
    AudioPictureRequest, Collage, CollageRequest, Frame, FrameStream, MediaService, PcmStream,
    ServiceError, ServiceResult,
};
use koo_core::time::{Time, TimeRange};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};

/// [`MediaService`] backed by the `ffmpeg` and `ffprobe` binaries.
pub struct FfmpegMedia {
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
    frames: Mutex<FrameCache>,
    peaks: Mutex<HashMap<(Fingerprint, u32), Arc<waveform::Peaks>>>,
    /// How this FFmpeg maps HDR to SDR, found on first use.
    tone_method: OnceLock<hdr::Method>,
}

impl FfmpegMedia {
    /// Locates the binaries and checks that they run.
    pub fn new() -> anyhow::Result<Self> {
        let find = |var: &str, name: &str| {
            std::env::var_os(var)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(name))
        };
        Self::with_binaries(
            find("KOOEDITOR_FFMPEG", "ffmpeg"),
            find("KOOEDITOR_FFPROBE", "ffprobe"),
        )
    }

    pub fn with_binaries(ffmpeg: PathBuf, ffprobe: PathBuf) -> anyhow::Result<Self> {
        for bin in [&ffmpeg, &ffprobe] {
            let ok = command(bin)
                .arg("-version")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !ok {
                anyhow::bail!("{} not found or not runnable", bin.display());
            }
        }
        Ok(Self {
            ffmpeg,
            ffprobe,
            frames: Mutex::new(FrameCache::default()),
            peaks: Mutex::new(HashMap::new()),
            tone_method: OnceLock::new(),
        })
    }

    fn ffmpeg(&self) -> Command {
        let mut c = command(&self.ffmpeg);
        c.args(["-hide_banner", "-loglevel", "error", "-nostdin"]);
        c
    }

    /// The filters mapping HDR pictures to SDR with this FFmpeg; `None`
    /// without a mapping.
    fn tone_filters(&self, mapping: Option<&hdr::Mapping>) -> ServiceResult<Option<String>> {
        let Some(mapping) = mapping else {
            return Ok(None);
        };
        let method = *self.tone_method.get_or_init(|| {
            let out = command(&self.ffmpeg)
                .args(["-hide_banner", "-filters"])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                .unwrap_or_default();
            hdr::Method::from_filters(&out)
        });
        mapping
            .filters(method)
            .map(Some)
            .map_err(|e| err("tone map", e))
    }

    /// `item.info`, or a fresh probe when the pool hasn't stored one yet.
    fn info(&self, item: &MediaItem) -> ServiceResult<MediaInfo> {
        match &item.info {
            Some(info) => Ok(info.clone()),
            None => self.probe(&item.path),
        }
    }

    fn fingerprint_of(&self, item: &MediaItem) -> ServiceResult<Fingerprint> {
        match &item.fingerprint {
            Some(fp) => Ok(fp.clone()),
            None => self.fingerprint(&item.path),
        }
    }
}

impl MediaService for FfmpegMedia {
    fn probe(&self, path: &Path) -> ServiceResult<MediaInfo> {
        probe::probe(self, path)
    }

    fn fingerprint(&self, path: &Path) -> ServiceResult<Fingerprint> {
        fingerprint::fingerprint(path)
    }

    fn frame(&self, item: &MediaItem, t: Time, max_width: u32) -> ServiceResult<Frame> {
        decode::frame(self, item, t, max_width)
    }

    fn playback(
        &self,
        item: &MediaItem,
        from: Time,
        max_width: u32,
    ) -> ServiceResult<Box<dyn FrameStream>> {
        decode::playback(self, item, from, max_width)
    }

    fn audio(
        &self,
        item: &MediaItem,
        stream: u32,
        from: Time,
        sample_rate: u32,
    ) -> ServiceResult<Box<dyn PcmStream>> {
        decode::audio(self, item, stream, from, sample_rate)
    }

    fn collage(&self, request: &CollageRequest) -> ServiceResult<Collage> {
        collage::collage(self, request)
    }

    fn waveform(
        &self,
        item: &MediaItem,
        stream: u32,
        range: TimeRange,
        buckets: usize,
    ) -> ServiceResult<Vec<f32>> {
        waveform::waveform(self, item, stream, range, buckets)
    }

    fn ensure_proxy(&self, item: &MediaItem, progress: &dyn Fn(f32)) -> ServiceResult<PathBuf> {
        proxy::ensure_proxy(self, item, progress)
    }

    fn audio_picture(&self, request: &AudioPictureRequest) -> ServiceResult<Frame> {
        spectrogram::audio_picture(self, request)
    }
}

/// A child process with no console window on Windows and no stdin.
fn command(bin: &Path) -> Command {
    let mut c = Command::new(bin);
    c.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

/// Seconds as FFmpeg reads them, to the microsecond.
fn secs_arg(t: Time) -> String {
    format!("{:.6}", t.as_secs_f64().max(0.0))
}

/// Last line FFmpeg wrote to stderr, for error messages.
fn stderr_tail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    text.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("unknown error")
        .trim()
        .to_string()
}

fn err(context: &str, e: impl std::fmt::Display) -> ServiceError {
    ServiceError(format!("{context}: {e}"))
}

/// Write `bytes` to `path` via a temporary sibling and a rename, so readers
/// never see a partial file.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = tmp_sibling(path);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// `name.ext` → `name.<pid>-<n>.tmp.ext`, unique per process and call.
fn tmp_sibling(path: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("tmp");
    let name = match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{stem}.{}-{n}.tmp.{ext}", std::process::id()),
        None => format!("{stem}.{}-{n}.tmp", std::process::id()),
    };
    path.with_file_name(name)
}

/// Still images are decoded without seeking.
fn is_still_image(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    matches!(
        ext.as_deref(),
        Some("png" | "jpg" | "jpeg" | "bmp" | "tif" | "tiff" | "webp" | "heic" | "avif")
    )
}
