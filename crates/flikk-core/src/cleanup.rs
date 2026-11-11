//! Sound cleaned at its source: Clean voice (a speech enhancement model)
//! and Noise Reduction (FFmpeg's `afftdn`). Neither runs live: each is
//! made once per source stream (and setting, and model), off the UI
//! thread, into a file in the cache ([`CleanupService`]), and the preview
//! mixer and the render both read that file, so they play the same
//! samples.
//!
//! What a clip plays ([`Cleanup::sound`]):
//!
//! ```text
//! base   = the recording, or its noise-reduced copy when the clip or its
//!          track has Noise Reduction
//! played = base, or with Clean voice on
//!          (1 − amount) · base + amount · the cleaned recording   (mix)
//! ```
//!
//! then the retime, the clip's other effects, its volume and fades as
//! before ([`crate::audiofx`]). Noise Reduction so works on the recording,
//! wherever it sits in a stack: the clip's settings first, then the
//! track's, applied to each clip on the track. Changing Clean voice's
//! amount needs no new copy: the mix is the same arithmetic in the mixer
//! ([`mix`]) and in FFmpeg ([`gains`] in a `pan`).
//!
//! Until its file is made a clip plays as recorded; a render makes it
//! first ([`CleanupService::complete`]).
//!
//! [`CleanupService`]: crate::services::CleanupService
//! [`CleanupService::complete`]: crate::services::CleanupService::complete

use crate::audiofx::{AudioFx, AudioFxKind};
use crate::model::{Clip, ClipContent, Fingerprint, MediaItem, MediaKind, Track};
use crate::paths;
use crate::services::{PcmStream, ServiceResult};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Folders under `paths::cache()` that hold the cleaned copies, swept
/// once unused for long ([`crate::cache_sweep`]).
pub const FOLDERS: [&str; 2] = [CLEANED, DENOISED];

const CLEANED: &str = "clean-voice";
const DENOISED: &str = "denoised";

/// A clip's Clean voice: on or off, and how much of the cleaned sound is
/// heard. Kept while off, so turning it back on finds the amount it had.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct VoiceCleanup {
    pub enabled: bool,
    /// The share of the cleaned sound, 0..1; the rest is the recording.
    pub amount: f32,
}

impl VoiceCleanup {
    /// Where a clip starts when Clean voice is first turned on: leaving
    /// some of the room in sounds more natural than full strength.
    pub const DEFAULT_AMOUNT: f32 = 0.8;

    /// On at `amount`, clamped to 0..1.
    pub fn on(amount: f32) -> Self {
        Self {
            enabled: true,
            amount: amount.clamp(0.0, 1.0),
        }
    }

    /// The amount heard: `None` while off.
    pub fn heard(&self) -> Option<f32> {
        self.enabled.then_some(self.amount.clamp(0.0, 1.0))
    }
}

impl Default for VoiceCleanup {
    fn default() -> Self {
        Self::on(Self::DEFAULT_AMOUNT)
    }
}

/// The gains of the base sound and of the cleaned one at `amount`: the
/// amount to 1/10 000, so the mixer and FFmpeg multiply by the same
/// numbers. They sum to 1.
pub fn gains(amount: f32) -> (f32, f32) {
    let cleaned = (amount.clamp(0.0, 1.0) * 10_000.0).round() / 10_000.0;
    (1.0 - cleaned, cleaned)
}

/// One sample of the base and the cleaned sound mixed at [`gains`].
pub fn mix(base: f32, cleaned: f32, (keep, gain): (f32, f32)) -> f32 {
    keep * base + gain * cleaned
}

/// Two streams of the same length mixed sample by sample ([`mix`]): what
/// the preview hears of a clip with Clean voice on.
pub struct Blend {
    base: Box<dyn PcmStream>,
    cleaned: Box<dyn PcmStream>,
    gains: (f32, f32),
    buf: Vec<f32>,
}

impl Blend {
    pub fn new(base: Box<dyn PcmStream>, cleaned: Box<dyn PcmStream>, amount: f32) -> Self {
        Self {
            base,
            cleaned,
            gains: gains(amount),
            buf: Vec::new(),
        }
    }
}

impl PcmStream for Blend {
    fn read(&mut self, out: &mut [f32]) -> ServiceResult<usize> {
        let n = self.base.read(out)?;
        self.buf.clear();
        self.buf.resize(n, 0.0);
        let mut got = 0;
        while got < n {
            match self.cleaned.read(&mut self.buf[got..])? {
                0 => break,
                k => got += k,
            }
        }
        for (o, c) in out[..n].iter_mut().zip(&self.buf) {
            *o = mix(*o, *c, self.gains);
        }
        Ok(n)
    }
}

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
    /// Clean voice: speech enhanced by a model.
    CleanVoice,
    /// Noise Reduction, these settings in order.
    Denoise(Vec<Denoise>),
}

impl Treatment {
    /// Where the copy of `stream` of the media with `fingerprint` that
    /// `model` makes is kept (its id; `afftdn` for Noise Reduction): a new
    /// model or setting is a new copy.
    pub fn file(&self, fingerprint: &Fingerprint, stream: u32, model: &str) -> PathBuf {
        let fp = fingerprint.file_stem();
        let model: String = model
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        match self {
            Treatment::CleanVoice => paths::cache()
                .join(CLEANED)
                .join(format!("{fp}-{stream}-{model}.flac")),
            Treatment::Denoise(settings) => {
                let key: Vec<String> = settings
                    .iter()
                    .map(|d| format!("{:.2}_{:.2}", d.reduction_db, d.floor_db))
                    .collect();
                paths::cache()
                    .join(DENOISED)
                    .join(format!("{fp}-{stream}-{model}-{}.flac", key.join("-")))
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
    /// Clean voice's amount, while it is on.
    pub clean_voice: Option<f32>,
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
#[derive(Clone, Debug, PartialEq)]
pub struct Sound {
    /// The base: the recording or its noise-reduced copy.
    pub file: SoundFile,
    /// The cleaned copy and its amount, mixed into the base ([`mix`]).
    pub cleaned: Option<(PathBuf, f32)>,
}

impl Cleanup {
    /// The cleaning `clip` gets on `track`: Clean voice while on, and the
    /// active Noise Reduction effects of the clip, then of the track.
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
            clean_voice: clip.audio.voice_cleanup.and_then(|v| v.heard()),
        }
    }

    /// Every copy this needs.
    pub fn treatments(&self) -> impl Iterator<Item = &Treatment> {
        let clean = self.clean_voice.map(|_| &Treatment::CleanVoice);
        self.denoise.iter().chain(clean)
    }

    /// What `clip` plays, `made` giving the file of each copy once it is
    /// made: the recording, without Clean voice, until they are.
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
        let cleaned = self
            .clean_voice
            .and_then(|amount| Some((made(&Treatment::CleanVoice)?, amount)));
        Sound { file, cleaned }
    }
}

/// Copies made, by the media's fingerprint, stream and treatment: what a
/// render hands its graph once it has made them.
#[derive(Clone, Debug, Default)]
pub struct Copies(Vec<(Fingerprint, u32, Treatment, PathBuf)>);

impl Copies {
    pub fn insert(&mut self, item: &MediaItem, stream: u32, treatment: &Treatment, file: PathBuf) {
        if let Some(fp) = &item.fingerprint
            && self.get(item, stream, treatment).is_none()
        {
            self.0.push((fp.clone(), stream, treatment.clone(), file));
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The file of the copy of `stream` of `item` that `treatment` makes.
    pub fn get(&self, item: &MediaItem, stream: u32, treatment: &Treatment) -> Option<PathBuf> {
        let fp = item.fingerprint.as_ref()?;
        self.0
            .iter()
            .find(|(f, s, t, _)| f == fp && *s == stream && t == treatment)
            .map(|(_, _, _, file)| file.clone())
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
