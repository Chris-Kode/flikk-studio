//! Program audio through `cpal`. A mixer thread follows the signal flow of
//! `koo_core::audiofx` (the render follows it too):
//!
//! * each audible clip span (the clip plus crossfade handles) opens a
//!   `MediaService::audio` stream as the play position reaches it, retimed
//!   with a pitch-preserving stretch when its speed is not 1, through the
//!   clip's effect chain, then its volume (gain or keyframes), fades and
//!   equal-power crossfade ramps;
//! * each audio track sums its spans, applies volume, pan and its effect
//!   chain, and reports its meter;
//! * the master sums the tracks, ducking music under the others, reports
//!   its meter and feeds a short queue drained by the output callback.
//!
//! Effect state lives in the voices and tracks, so it starts fresh with
//! every playback start and seek (both rebuild the mixer). Without an
//! output device the mixer runs on the wall clock so meters still move.

use crate::ui::mixer::meter::{self, Level};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use koo_core::audiofx::dsp::Chain;
use koo_core::audiofx::stretch::Stretch;
use koo_core::audiofx::{self, Ramp};
use koo_core::model::{Clip, ClipContent, MediaItem, Project, Property, TrackKind};
use koo_core::services::{MediaService, PcmStream};
use koo_core::time::FLICKS_PER_SECOND;
use koo_core::{ClipId, Time, TimeRange, TrackId};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
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

pub struct AudioOut {
    stop: Arc<AtomicBool>,
}

impl AudioOut {
    pub fn start(media: Arc<dyn MediaService>, project: Arc<Project>, from: Time) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let _ = thread::Builder::new()
            .name("koo-audio".into())
            .spawn(move || run(flag, media, project, from));
        Self { stop }
    }
}

impl Drop for AudioOut {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

type Queue = Arc<Mutex<VecDeque<f32>>>;

fn run(stop: Arc<AtomicBool>, media: Arc<dyn MediaService>, project: Arc<Project>, from: Time) {
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
    let mut mixer = Mixer::new(media, &project, from, rate);
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

/// One clip span to play: see `koo_core::audiofx::Span`.
struct Source {
    clip: Clip,
    range: TimeRange,
    ramps: Vec<Ramp>,
    item: MediaItem,
    stream: u32,
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
    stretch: Option<Stretch>,
    chain: Chain,
    /// Frames of silence before the source's first sample.
    lead: usize,
    /// Output frames to drop for the chain's look-ahead.
    discard: usize,
    tmp: Vec<f32>,
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
    media: Arc<dyn MediaService>,
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
    /// Per-sample coefficient of the meters' RMS window.
    rms_coef: f32,
}

/// Music under dialogue, like the render's `sidechaincompress`: when the
/// other tracks pass the threshold the music eases down to `reduction`.
struct Duck {
    track: TrackId,
    reduction: f32,
    attack: f32,
    release: f32,
    /// Smoothed level of the other tracks.
    level: f32,
    gain: f32,
}

/// Linear threshold of the render's compressor.
const DUCK_THRESHOLD: f32 = 0.015;
/// RMS window of the meters.
const RMS_WINDOW_S: f64 = 0.3;

impl Mixer {
    fn new(media: Arc<dyn MediaService>, project: &Project, from: Time, rate: u32) -> Self {
        let tracks = &project.timeline.tracks;
        let solo = tracks
            .iter()
            .any(|t| t.kind == TrackKind::Audio && t.mix.solo);
        let buses: Vec<Bus> = tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Audio && !t.mix.mute && (!solo || t.mix.solo))
            .map(|t| {
                let sources = audiofx::spans(t)
                    .into_iter()
                    .filter(|s| s.range.end > from)
                    .filter_map(|s| {
                        let c = s.clip;
                        let item = project.media(c.media()?)?.clone();
                        let stream = match &c.content {
                            ClipContent::Media { audio_stream, .. } => audio_stream
                                .or_else(|| item.info.as_ref()?.audio.first().map(|a| a.index))
                                .unwrap_or(0),
                            _ => return None,
                        };
                        Some(Source {
                            clip: c.clone(),
                            range: s.range,
                            ramps: s.ramps,
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
        // Per-sample smoothing over a time constant.
        let coef = |t: Time| (-1. / (t.as_secs_f64().max(1e-4) * rate as f64)).exp() as f32;
        let duck = project.timeline.ducking.as_ref().and_then(|d| {
            let music = buses
                .iter()
                .any(|b| b.id == d.music_track && !b.sources.is_empty());
            let others = buses
                .iter()
                .any(|b| b.id != d.music_track && !b.sources.is_empty());
            (music && others).then(|| Duck {
                track: d.music_track,
                reduction: db(-d.reduction_db.abs()),
                attack: coef(d.attack),
                release: coef(d.release),
                level: 0.,
                gain: 1.,
            })
        });
        Self {
            media,
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
        let source_t = clip.to_source(at)?;
        // Handles before the media's first sample play as silence.
        let lead = if source_t < Time::ZERO {
            ((-source_t).as_secs_f64() / clip.speed * self.rate as f64).round() as usize
        } else {
            0
        };
        let stream = self
            .media
            .audio(
                &source.item,
                source.stream,
                source_t.max(Time::ZERO),
                self.rate,
            )
            .ok()?;
        let chain = Chain::new(&clip.audio.effects, self.rate);
        Some(Voice {
            stream,
            stretch: ((clip.speed - 1.).abs() > 1e-6).then(|| Stretch::new(clip.speed, self.rate)),
            discard: chain.latency(),
            chain,
            lead,
            tmp: vec![0.; BLOCK * 2],
        })
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
                let key = o[0].abs().max(o[1].abs());
                let k = if key > d.level { d.attack } else { d.release };
                d.level = key + (d.level - key) * k;
                let target = if d.level > DUCK_THRESHOLD {
                    d.reduction
                } else {
                    1.
                };
                let k = if target < d.gain { d.attack } else { d.release };
                d.gain = target + (d.gain - target) * k;
                o[0] += m[0] * d.gain;
                o[1] += m[1] * d.gain;
            }
        } else {
            for (o, m) in out.iter_mut().zip(&self.music) {
                *o += m;
            }
        }
        self.master_rms = measure(out, &self.master, self.master_rms, self.rms_coef);
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

/// Time of mixed frame `frame` of a mix started at `from`.
fn time_at(from: Time, rate: u32, frame: u64) -> Time {
    from + Time((frame as i128 * FLICKS_PER_SECOND as i128 / rate as i128) as i64)
}

fn db(db: f32) -> f32 {
    10f32.powf(db / 20.)
}
