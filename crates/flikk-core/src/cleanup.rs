//! Sound cleaned at its source. Noise Reduction (FFmpeg's `afftdn`) does
//! not run live: it is made once per source stream and setting, off the UI
//! thread, into a file in the cache ([`CleanupService`]), and the preview
//! mixer and the render both read that file, so they play the same
//! samples.
//!
//! What a clip plays ([`Cleanup::sound`]): its recording, or the
//! noise-reduced copy when the clip or its track has Noise Reduction. The
//! retime, the clip's other effects, its volume and fades follow as before
//! ([`crate::audiofx`]). Noise Reduction so works on the recording,
//! wherever it sits in a stack: the clip's settings first, then the
//! track's, applied to each clip on the track.
//!
//! Until its file is made a clip plays as recorded; a render makes it
//! first ([`CleanupService::complete`]).
//!
//! [`CleanupService`]: crate::services::CleanupService
//! [`CleanupService::complete`]: crate::services::CleanupService::complete

use crate::audiofx::{AudioFx, AudioFxKind};
use crate::model::{Clip, ClipContent, Fingerprint, MediaItem, MediaKind, Track};
use crate::paths;
use std::path::PathBuf;

/// Folders under `paths::cache()` that hold the cleaned copies, swept
/// once unused for long ([`crate::cache_sweep`]).
pub const FOLDERS: [&str; 1] = [DENOISED];

const DENOISED: &str = "denoised";

/// Samples per second of the copies: what FFmpeg resamples every source
/// to before it cleans it.
pub const RATE: u32 = 48_000;

/// One Noise Reduction effect's settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Denoise {
    /// How far noise is lowered, dB (`afftdn`'s `nr`).
    pub reduction_db: f64,
    /// The level noise is measured from, dB (`afftdn`'s `nf`).
    pub floor_db: f64,
}

impl Denoise {
    /// The settings of `fx` when it is an active Noise Reduction.
    fn of(fx: &AudioFx) -> Option<Self> {
        (fx.kind == AudioFxKind::NoiseReduction && fx.is_active()).then(|| Self {
            reduction_db: fx.value("amount_db"),
            floor_db: fx.value("floor_db"),
        })
    }
}

/// How a copy of a source stream is made.
#[derive(Clone, Debug, PartialEq)]
pub enum Treatment {
    /// Noise Reduction, these settings in order.
    Denoise(Vec<Denoise>),
}

impl Treatment {
    /// Where the copy of `stream` of the media with `fingerprint` is kept.
    pub fn file(&self, fingerprint: &Fingerprint, stream: u32) -> PathBuf {
        let fp = fingerprint.file_stem();
        match self {
            Treatment::Denoise(settings) => {
                let key: Vec<String> = settings
                    .iter()
                    .map(|d| format!("{:.2}_{:.2}", d.reduction_db, d.floor_db))
                    .collect();
                paths::cache()
                    .join(DENOISED)
                    .join(format!("{fp}-{stream}-afftdn-{}.flac", key.join("-")))
            }
        }
    }
}

/// The audio stream of `item` that `clip` plays: the one it names, else
/// the item's first, as a container index (or a count from zero when the
/// item was never probed).
pub fn stream_of(clip: &Clip, item: &MediaItem) -> u32 {
    match &clip.content {
        ClipContent::Media { audio_stream, .. } => *audio_stream,
        _ => None,
    }
    .or_else(|| item.first_audio_stream())
    .unwrap_or(0)
}

/// Whether `item` has sound to clean: not an image, and not a file
/// probed without audio.
pub fn has_sound(item: &MediaItem) -> bool {
    item.kind != MediaKind::Image && item.info.as_ref().is_none_or(|i| !i.audio.is_empty())
}

/// What a clip is cleaned with ([`Cleanup::of`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cleanup {
    pub denoise: Option<Treatment>,
}

/// Where a clip's sound is read from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SoundFile {
    /// The media item's own file, at the clip's audio stream (`None`: the
    /// item's first).
    Recording { stream: Option<u32> },
    /// A cleaned copy in the cache, its one audio stream.
    Copy(PathBuf),
}

/// What a clip plays, before its retime and effects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sound {
    pub file: SoundFile,
}

impl Cleanup {
    /// The cleaning `clip` gets on `track`: the active Noise Reduction
    /// effects of the clip, then of the track.
    pub fn of(clip: &Clip, track: &Track) -> Self {
        let settings: Vec<Denoise> = clip
            .audio
            .effects
            .iter()
            .chain(&track.mix.effects)
            .filter_map(Denoise::of)
            .collect();
        Self {
            denoise: (!settings.is_empty()).then_some(Treatment::Denoise(settings)),
        }
    }

    /// Every copy this needs.
    pub fn treatments(&self) -> impl Iterator<Item = &Treatment> {
        self.denoise.iter()
    }

    /// What `clip` plays, `made` giving the file of each copy once it is
    /// made: the recording until it is.
    pub fn sound(&self, clip: &Clip, mut made: impl FnMut(&Treatment) -> Option<PathBuf>) -> Sound {
        let recording = SoundFile::Recording {
            stream: match &clip.content {
                ClipContent::Media { audio_stream, .. } => *audio_stream,
                _ => None,
            },
        };
        let file = self
            .denoise
            .as_ref()
            .and_then(&mut made)
            .map_or(recording, SoundFile::Copy);
        Sound { file }
    }
}

/// How far a copy is made.
#[derive(Clone, Debug, PartialEq)]
pub enum Progress {
    /// Waiting its turn, or being made: 0..1.
    Making(f32),
    Made(PathBuf),
    /// Cannot be made here (no fingerprint, no model).
    Unavailable(String),
    /// Failed lately; asked for again later, it is tried again.
    Failed(String),
}

impl Progress {
    /// The file, once made.
    pub fn made(self) -> Option<PathBuf> {
        match self {
            Progress::Made(path) => Some(path),
            _ => None,
        }
    }
}
