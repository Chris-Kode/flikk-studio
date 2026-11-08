//! Media engine on FFmpeg: probe, fingerprint, proxies, frame decode,
//! collages, waveforms and audio pictures. Implements `flikk_core::services::MediaService`.
//!
//! FFmpeg is used as a pair of binaries (`FLIKK_FFMPEG` /
//! `FLIKK_FFPROBE`, else `PATH`), never linked. Every call blocks; callers
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
use flikk_core::hdr;
use flikk_core::model::{Fingerprint, MediaInfo, MediaItem};
use flikk_core::services::{
    AudioPictureRequest, Collage, CollageRequest, Frame, FrameStream, MediaService, PcmStream,
    ServiceError, ServiceResult,
};
use flikk_core::time::{Time, TimeRange};
use flikk_deliver::ffmpeg::ToneMethodProbe;
use flikk_os::process::command;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

/// [`MediaService`] backed by the `ffmpeg` and `ffprobe` binaries.
pub struct FfmpegMedia {
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
    frames: Mutex<FrameCache>,
    peaks: Mutex<HashMap<(Fingerprint, u32), Arc<waveform::Peaks>>>,
    /// How this FFmpeg maps HDR to SDR, found on first use.
    tone_method: ToneMethodProbe,
    /// Files hardware decoding failed on: playback decodes them in software.
    software_only: Arc<Mutex<HashSet<PathBuf>>>,
}

impl FfmpegMedia {
    /// Locates the binaries and checks that they run.
    pub fn new() -> anyhow::Result<Self> {
        Self::with_binaries(flikk_os::process::ffmpeg(), flikk_os::process::ffprobe())
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
            tone_method: ToneMethodProbe::new(),
            software_only: Arc::default(),
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
        mapping
            .filters(self.tone_method.method(&self.ffmpeg))
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

/// Seconds as FFmpeg reads them, to the microsecond.
fn secs_arg(t: Time) -> String {
    format!("{:.6}", t.as_secs_f64().max(0.0))
}

/// The last non-empty line FFmpeg wrote to stderr, for error messages;
/// "unknown error" when it wrote none.
fn last_stderr_line(stderr: &[u8]) -> String {
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
