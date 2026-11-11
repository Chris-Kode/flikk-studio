//! Program audio through `cpal`. A mixer thread follows the signal flow of
//! `flikk_core::audiofx` (the render follows it too):
//!
//! * each audible clip span (the clip plus crossfade handles) opens a
//!   `MediaService::audio` stream as the play position reaches it (its
//!   cleaned copy once made, `flikk_core::cleanup`), retimed
//!   with a pitch-preserving stretch when its speed is not 1, through the
//!   clip's effect chain, then its volume (gain or keyframes), fades and
//!   equal-power crossfade ramps;
//! * each audio track sums its spans, applies volume, pan and its effect
//!   chain, and reports its meter;
//! * the master sums the tracks, ducking music under the others, reports
//!   its meter and each side's for the viewer ([`channels`]), and feeds a
//!   short queue drained by the output callback.
//!
//! Effect state lives in the voices and tracks, so it starts fresh with
//! every playback start and seek (both rebuild the mixer). Without an
//! output device the mixer runs on the wall clock so meters still move.

use crate::ui::mixer::meter::{self, Level};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use flikk_core::audiofx::dsp::{self, Chain, Curve};
use flikk_core::audiofx::stretch::Stretch;
use flikk_core::audiofx::{self, Ramp};
use flikk_core::cleanup::{self, Cleanup, SoundFile};
use flikk_core::model::{Clip, ClipContent, MediaItem, Project, Property};
use flikk_core::services::{CleanupService, MediaService, PcmStream};
use flikk_core::speed::RampAudio;
use flikk_core::sync::varispeed::{Varispeed, is_varispeed};
use flikk_core::time::FLICKS_PER_SECOND;
use flikk_core::{ClipId, Time, TimeRange, TrackId};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Frames mixed per block.
const BLOCK: usize = 1024;
/// How far ahead of the output the mixer stays, in seconds.
const AHEAD: f64 = 0.12;
/// Frames between gain evaluations; gains are interpolated in between.
const GAIN_STEP: usize = 32;
/// Rate of the clock-driven mixer when there is no output device.
const FALLBACK_RATE: u32 = 48_000;

/// The master's left and right meters.
static CHANNELS: LazyLock<[Level; 2]> = LazyLock::new(Default::default);

/// The master's left and right meters, for the viewer.
pub(super) fn channels() -> &'static [Level; 2] {
    &CHANNELS
}

pub struct AudioOut {
    stop: Arc<AtomicBool>,
}

/// The engines the mixer reads sound through.
#[derive(Clone)]
pub struct Engines {
    pub media: Arc<dyn MediaService>,
    /// Cleaned copies of source sound; without it clips play as recorded.
    pub cleanup: Option<Arc<dyn CleanupService>>,
}

impl AudioOut {
    pub fn start(engines: Engines, project: Arc<Project>, from: Time) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let _ = thread::Builder::new()
            .name("flikk-audio".into())
            .spawn(move || run(flag, engines, project, from));
        Self { stop }
    }
}

impl Drop for AudioOut {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

type Queue = Arc<Mutex<VecDeque<f32>>>;

fn run(stop: Arc<AtomicBool>, engines: Engines, project: Arc<Project>, from: Time) {
    let host = cpal::default_host();
    let output = host.default_output_device().and_then(|device| {
        let config = device.default_output_config().ok()?;
        let rate = config.sample_rate().0;
        let channels = config.channels() as usize;
        let queue: Queue = Arc::new(Mutex::new(VecDeque::new()));
        let stream = match config.sample_format() {
            SampleFormat::F32 => build::<f32>(&device, &config.into(), channels, queue.clone()),
            SampleFormat::I16 => build::<i16>(&device, &config.into(), channels, queue.clone()),
            SampleFormat::U16 => build::<u16>(&device, &config.into(), channels, queue.clone()),
            _ => None,
        }?;
        stream.play().ok()?;
        Some((stream, queue, rate))
    });
    let rate = output.as_ref().map_or(FALLBACK_RATE, |o| o.2);
    let mut mixer = Mixer::new(engines, &project, from, rate);
    let ahead = (AHEAD * rate as f64) as usize * 2;
    let mut block = vec![0f32; BLOCK * 2];
    let started = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        let wait = match &output {
            Some((_, queue, _)) => queue.lock().map(|q| q.len()).unwrap_or(0) >= ahead,
            None => {
                let due = (started.elapsed().as_secs_f64() + AHEAD) * rate as f64;
                mixer.position as f64 >= due
            }
        };
        if wait {
            thread::sleep(Duration::from_millis(4));
            continue;
        }
        mixer.mix(&mut block);
        if let Some((_, queue, _)) = &output
            && let Ok(mut q) = queue.lock()
        {
            q.extend(block.iter().copied());
        }
    }
    drop(output);
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    queue: Queue,
) -> Option<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
{
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| {
                // Never block the audio thread on the mixer: play silence instead.
                let mut q = queue.try_lock().ok();
                for frame in data.chunks_mut(channels) {
                    let (l, r) = match q.as_mut() {
                        Some(q) => (q.pop_front().unwrap_or(0.), q.pop_front().unwrap_or(0.)),
                        None => (0., 0.),
                    };
                    for (i, s) in frame.iter_mut().enumerate() {
                        let v = match (channels, i) {
                            (1, _) => (l + r) * 0.5,
                            (_, 0) => l,
                            (_, 1) => r,
                            _ => 0.,
                        };
                        *s = T::from_sample(v);
                    }
                }
            },
            |_| {},
            None,
        )
        .ok()
}

/// One clip span to play: see `flikk_core::audiofx::Span`.
struct Source {
    clip: Clip,
    range: TimeRange,
    ramps: Vec<Ramp>,
    item: MediaItem,
    stream: u32,
    cleanup: Cleanup,
    /// Volume keyframes present: evaluate the volume per step.
    automated: bool,
}

impl Source {
    /// Linear gain at `t`: volume, fades and ramps.
    fn gain(&self, t: Time) -> f32 {
        let volume = if self.automated {
            self.clip.value_at(Property::Volume, t - self.clip.start)
        } else {
            self.clip.audio.gain_db
        };
        (db(volume) as f64 * audiofx::envelope(&self.clip, self.range, &self.ramps, t)) as f32
    }
}

/// An open span: its stream, retime, effects and the silence still owed
/// before the media starts.
struct Voice {
    stream: Box<dyn PcmStream>,
    stretch: Option<Retime>,
    chain: Chain,
    /// Frames of silence before the source's first sample.
    lead: usize,
    /// Output frames to drop for the chain's look-ahead.
    discard: usize,
    tmp: Vec<f32>,
}

/// How a retimed clip plays: resampled for the tiny speed changes that
/// correct clock drift (as the render does), else time-stretched.
enum Retime {
    Stretch(Stretch),
    Varispeed(Varispeed),
}

impl Retime {
    fn new(speed: f64, rate: u32) -> Option<Self> {
        if is_varispeed(speed) {
            Some(Self::Varispeed(Varispeed::new(speed)))
        } else {
            ((speed - 1.).abs() > 1e-6).then(|| Self::Stretch(Stretch::new(speed, rate)))
        }
    }

    fn read(&mut self, out: &mut [f32], pull: &mut dyn FnMut(&mut [f32]) -> usize) {
        match self {
            Self::Stretch(s) => s.read(out, pull),
            Self::Varispeed(v) => v.read(out, pull),
        }
    }
}

impl Voice {
    /// Raw (retimed) source frames into `out`.
    fn fill(&mut self, out: &mut [f32]) {
        let silent = (self.lead * 2).min(out.len());
        out[..silent].fill(0.);
        self.lead -= silent / 2;
        let rest = &mut out[silent..];
        let stream = &mut self.stream;
        let mut pull = |o: &mut [f32]| stream.read(o).unwrap_or_default();
        match self.stretch.as_mut() {
            Some(s) => s.read(rest, &mut pull),
            None => {
                let mut filled = 0;
                while filled < rest.len() {
                    match pull(&mut rest[filled..]) {
                        0 => break,
                        n => filled += n,
                    }
                }
                rest[filled..].fill(0.);
            }
        }
    }

    /// Processed frames into `out`.
    fn render(&mut self, out: &mut [f32]) {
        while self.discard > 0 {
            let mut tmp = std::mem::take(&mut self.tmp);
            let n = (self.discard * 2).min(tmp.len());
            self.fill(&mut tmp[..n]);
            self.chain.process(&mut tmp[..n]);
            self.discard -= n / 2;
            self.tmp = tmp;
        }
        self.fill(out);
        self.chain.process(out);
    }
}

/// Where a ramped clip's source is when its stretch, starting at timeline
/// time `from`, has made output frame `o`: input frames from there.
fn ramp_stretch(clip: &Clip, from: Time, rate: u32) -> Stretch {
    let clip = clip.clone();
    let s0 = clip.to_source(from).unwrap_or(Time::ZERO).max(Time::ZERO);
    let rate = rate as f64;
    Stretch::mapped(
        rate as u32,
        Box::new(move |o| {
            let t = from + Time::from_secs_f64(o / rate);
            let s = clip.to_source(t).unwrap_or(s0);
            (s - s0).as_secs_f64() * rate
        }),
    )
}

/// The source time at a media clip's start.
fn clip_in(clip: &Clip) -> Time {
    match &clip.content {
        ClipContent::Media { source_in, .. } => *source_in,
        _ => Time::ZERO,
    }
}

/// One audio track: its spans, mix settings, effects and meter.
struct Bus {
    id: TrackId,
    sources: Vec<Source>,
    gain: f32,
    pan: f32,
    chain: Chain,
    /// The chain's look-ahead: the track mixes its clips this many frames
    /// early so its output lines up.
    lead: u64,
    meter: Arc<Level>,
    rms: f32,
    buf: Vec<f32>,
}

struct Mixer {
    engines: Engines,
    buses: Vec<Bus>,
    voices: HashMap<ClipId, Voice>,
    finished: HashSet<ClipId>,
    from: Time,
    rate: u32,
    /// Frames mixed so far.
    position: u64,
    scratch: Vec<f32>,
    duck: Option<Duck>,
    /// The ducked track's block, mixed in after the gain is applied.
    music: Vec<f32>,
    master: Arc<Level>,
    master_rms: f32,
    /// Running mean squares of the left and right meters.
    channel_ms: [f32; 2],
    /// Per-sample coefficient of the meters' RMS window.
    rms_coef: f32,
}

/// Music under dialogue: the music track compressed with the other tracks
/// as its key, the same curve as the render's `sidechaincompress`.
struct Duck {
    track: TrackId,
    curve: Curve,
}
/// RMS window of the meters.
const RMS_WINDOW_S: f64 = 0.3;

impl Mixer {
    fn new(engines: Engines, project: &Project, from: Time, rate: u32) -> Self {
        let tl = &project.timeline;
        let buses: Vec<Bus> = tl
            .tracks
            .iter()
            .filter(|t| tl.audible(t))
            .map(|t| {
                let sources = audiofx::spans(t)
                    .into_iter()
                    .filter(|s| s.range.end > from)
                    .filter_map(|s| {
                        let c = s.clip;
                        let item = project.media(c.media()?)?.clone();
                        let stream = cleanup::stream_of(c, &item);
                        Some(Source {
                            clip: c.clone(),
                            range: s.range,
                            ramps: s.ramps,
                            cleanup: Cleanup::of(c, t),
                            item,
                            stream,
                            automated: c.keyframes.iter().any(|k| k.property == Property::Volume),
                        })
                    })
                    .collect();
                let chain = Chain::new(&t.mix.effects, rate);
                Bus {
                    id: t.id,
                    sources,
                    gain: db(t.mix.volume_db),
                    pan: t.mix.pan.clamp(-1., 1.),
                    lead: chain.latency() as u64,
                    chain,
                    meter: meter::level(Some(t.id)),
                    rms: 0.,
                    buf: vec![0.; BLOCK * 2],
                }
            })
            .collect();
        let duck = project.timeline.ducking.as_ref().and_then(|d| {
            let music = buses
                .iter()
                .any(|b| b.id == d.music_track && !b.sources.is_empty());
            let others = buses
                .iter()
                .any(|b| b.id != d.music_track && !b.sources.is_empty());
            let (threshold, ratio) = dsp::ducking(d.reduction_db as f64);
            let ms = |t: Time| t.as_secs_f64() * 1000.;
            (music && others).then(|| Duck {
                track: d.music_track,
                curve: Curve::new(threshold, ratio, ms(d.attack), ms(d.release), rate as f64),
            })
        });
        Self {
            engines,
            buses,
            voices: HashMap::new(),
            finished: HashSet::new(),
            from,
            rate,
            position: 0,
            scratch: Vec::new(),
            duck,
            music: Vec::new(),
            master: meter::level(None),
            master_rms: 0.,
            channel_ms: [0.; 2],
            rms_coef: (-1. / (RMS_WINDOW_S * rate as f64)).exp() as f32,
        }
    }

    fn time(&self, frame: u64) -> Time {
        time_at(self.from, self.rate, frame)
    }

    /// Frames from `t0` to `t`, clamped to `0..=frames`.
    fn frames_to(&self, t0: Time, t: Time, frames: usize) -> usize {
        if t <= t0 {
            return 0;
        }
        (((t - t0).0 as i128 * self.rate as i128 / FLICKS_PER_SECOND as i128) as usize).min(frames)
    }

    /// Open the voice for `source` at timeline time `at`.
    fn open(&self, source: &Source, at: Time) -> Option<Voice> {
        let clip = &source.clip;
        if clip.is_ramped() && clip.retime.audio == RampAudio::Mute {
            return None;
        }
        let source_t = clip.to_source(at)?;
        // Handles before the media's first sample play as silence.
        let lead_t = if source_t < Time::ZERO {
            (clip.start + clip.timeline_offset(-clip_in(clip)) - at).max(Time::ZERO)
        } else {
            Time::ZERO
        };
        let lead = (lead_t.as_secs_f64() * self.rate as f64).round() as usize;
        let stream = self.sound(source, source_t.max(Time::ZERO))?;
        let chain = Chain::new(&clip.audio.effects, self.rate);
        Some(Voice {
            stream,
            stretch: if clip.is_ramped() {
                Some(Retime::Stretch(ramp_stretch(clip, at + lead_t, self.rate)))
            } else {
                Retime::new(clip.speed, self.rate)
            },
            discard: chain.latency(),
            chain,
            lead,
            tmp: vec![0.; BLOCK * 2],
        })
    }

    /// `source`'s sound from source time `from`: its cleaned copy when it
    /// is made, else the recording.
    fn sound(&self, source: &Source, from: Time) -> Option<Box<dyn PcmStream>> {
        let sound = source.cleanup.sound(&source.clip, |treatment| {
            let service = self.engines.cleanup.as_ref()?;
            service
                .request(&source.item, source.stream, treatment)
                .made()
        });
        let media = &self.engines.media;
        match sound.file {
            SoundFile::Recording { .. } => {
                media.audio(&source.item, source.stream, from, self.rate)
            }
            SoundFile::Copy(path) => media.audio_file(&path, from, self.rate),
        }
        .ok()
    }

    /// Mix the next block into `out` (interleaved stereo).
    fn mix(&mut self, out: &mut [f32]) {
        out.fill(0.);
        self.music.clear();
        self.music.resize(out.len(), 0.);
        let ducked = self.duck.as_ref().map(|d| d.track);
        let frames = out.len() / 2;
        let mut buses = std::mem::take(&mut self.buses);
        for bus in &mut buses {
            bus.buf.clear();
            bus.buf.resize(out.len(), 0.);
            let base = self.position + bus.lead;
            let (t0, t1) = (self.time(base), self.time(base + frames as u64));
            for source in &bus.sources {
                let id = source.clip.id;
                if source.range.start >= t1 || source.range.end <= t0 || self.finished.contains(&id)
                {
                    continue;
                }
                let offset = self.frames_to(t0, source.range.start, frames);
                let end = if source.range.end < t1 {
                    self.frames_to(t0, source.range.end, frames)
                } else {
                    frames
                };
                if !self.voices.contains_key(&id) {
                    let at = self.time(base + offset as u64).max(source.range.start);
                    match self.open(source, at) {
                        Some(v) => {
                            self.voices.insert(id, v);
                        }
                        None => {
                            self.finished.insert(id);
                            continue;
                        }
                    }
                }
                if end > offset {
                    let n = end - offset;
                    self.scratch.resize(n * 2, 0.);
                    let voice = self.voices.get_mut(&id).expect("opened");
                    voice.render(&mut self.scratch);
                    let gain_at = |i: usize| {
                        source.gain(time_at(self.from, self.rate, base + (offset + i) as u64))
                    };
                    let mut i = 0;
                    let mut g0 = gain_at(0);
                    while i < n {
                        let step = GAIN_STEP.min(n - i);
                        let g1 = gain_at(i + step);
                        for k in 0..step {
                            let g = g0 + (g1 - g0) * k as f32 / step as f32;
                            let (o, s) = ((offset + i + k) * 2, (i + k) * 2);
                            bus.buf[o] += self.scratch[s] * g;
                            bus.buf[o + 1] += self.scratch[s + 1] * g;
                        }
                        g0 = g1;
                        i += step;
                    }
                }
                if source.range.end <= t1 {
                    self.voices.remove(&id);
                    self.finished.insert(id);
                }
            }
            let (pl, pr) = (
                (1. - bus.pan).min(1.) * bus.gain,
                (1. + bus.pan).min(1.) * bus.gain,
            );
            for f in bus.buf.as_chunks_mut::<2>().0.iter_mut() {
                f[0] *= pl;
                f[1] *= pr;
            }
            bus.chain.process(&mut bus.buf);
            bus.rms = measure(&bus.buf, &bus.meter, bus.rms, self.rms_coef);
            let dest = if ducked == Some(bus.id) {
                &mut self.music
            } else {
                &mut *out
            };
            for (d, s) in dest.iter_mut().zip(&bus.buf) {
                *d += s;
            }
        }
        self.buses = buses;
        if let Some(d) = self.duck.as_mut() {
            for (o, m) in out
                .as_chunks_mut::<2>()
                .0
                .iter_mut()
                .zip(self.music.as_chunks::<2>().0)
            {
                let key = (o[0].abs() + o[1].abs()) as f64 / 2.;
                let g = d.curve.gain(key) as f32;
                o[0] += m[0] * g;
                o[1] += m[1] * g;
            }
        } else {
            for (o, m) in out.iter_mut().zip(&self.music) {
                *o += m;
            }
        }
        self.master_rms = measure(out, &self.master, self.master_rms, self.rms_coef);
        measure_sides(out, &mut self.channel_ms, self.rms_coef);
        for s in out.iter_mut() {
            *s = s.clamp(-1., 1.);
        }
        self.position += frames as u64;
    }
}

/// Store a block's peak and running RMS (mean square in, mean square out).
fn measure(buf: &[f32], level: &Level, mut ms: f32, coef: f32) -> f32 {
    let mut peak = 0f32;
    for f in buf.as_chunks::<2>().0 {
        let (l, r) = (f[0].abs(), f[1].abs());
        peak = peak.max(l).max(r);
        let x = (l * l + r * r) * 0.5;
        ms = x + (ms - x) * coef;
    }
    level.store(peak, ms.sqrt());
    ms
}

/// Store a block's peak and running RMS per side into [`channels`].
fn measure_sides(buf: &[f32], ms: &mut [f32; 2], coef: f32) {
    let mut peak = [0f32; 2];
    for f in buf.as_chunks::<2>().0 {
        for ((p, m), x) in peak.iter_mut().zip(ms.iter_mut()).zip(f) {
            let x = x.abs();
            *p = p.max(x);
            *m = x * x + (*m - x * x) * coef;
        }
    }
    for (level, (p, m)) in CHANNELS.iter().zip(peak.iter().zip(ms.iter())) {
        level.store(*p, m.sqrt());
    }
}

/// Time of mixed frame `frame` of a mix started at `from`.
fn time_at(from: Time, rate: u32, frame: u64) -> Time {
    from + Time((frame as i128 * FLICKS_PER_SECOND as i128 / rate as i128) as i64)
}

fn db(db: f32) -> f32 {
    10f32.powf(db / 20.)
}
