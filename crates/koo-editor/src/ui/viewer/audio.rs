//! Program audio through `cpal`: a mixer thread opens a
//! `MediaService::audio` stream per audible clip as the play position reaches
//! it, applies clip gain, fades, track volume, pan, mute and solo, and feeds a
//! short queue drained by the output callback.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use koo_core::model::{Clip, MediaItem, Project, TrackKind};
use koo_core::services::{MediaService, PcmStream};
use koo_core::time::FLICKS_PER_SECOND;
use koo_core::{ClipId, Time};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Frames mixed per block.
const BLOCK: usize = 1024;
/// How far ahead of the output the mixer stays, in seconds.
const AHEAD: f64 = 0.12;

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
    let Some(device) = host.default_output_device() else {
        return;
    };
    let Ok(config) = device.default_output_config() else {
        return;
    };
    let rate = config.sample_rate().0;
    let channels = config.channels() as usize;
    let queue: Queue = Arc::new(Mutex::new(VecDeque::new()));
    let stream = match config.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, &config.into(), channels, queue.clone()),
        SampleFormat::I16 => build::<i16>(&device, &config.into(), channels, queue.clone()),
        SampleFormat::U16 => build::<u16>(&device, &config.into(), channels, queue.clone()),
        _ => return,
    };
    let Some(stream) = stream else { return };
    if stream.play().is_err() {
        return;
    }

    let mut mixer = Mixer::new(media, &project, from, rate);
    let ahead = (AHEAD * rate as f64) as usize * 2;
    let mut block = vec![0f32; BLOCK * 2];
    while !stop.load(Ordering::Relaxed) {
        let queued = queue.lock().map(|q| q.len()).unwrap_or(0);
        if queued >= ahead {
            thread::sleep(Duration::from_millis(4));
            continue;
        }
        mixer.mix(&mut block);
        if let Ok(mut q) = queue.lock() {
            q.extend(block.iter().copied());
        }
    }
    drop(stream);
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

struct Source {
    clip: Clip,
    item: MediaItem,
    stream: u32,
    /// Linear gain from clip and track volume.
    gain: f32,
    pan: f32,
}

struct Mixer {
    media: Arc<dyn MediaService>,
    sources: Vec<Source>,
    voices: HashMap<ClipId, Box<dyn PcmStream>>,
    finished: HashSet<ClipId>,
    from: Time,
    rate: u32,
    /// Frames mixed so far.
    position: u64,
    scratch: Vec<f32>,
}

impl Mixer {
    fn new(media: Arc<dyn MediaService>, project: &Project, from: Time, rate: u32) -> Self {
        let tracks = &project.timeline.tracks;
        let solo = tracks
            .iter()
            .any(|t| t.kind == TrackKind::Audio && t.mix.solo);
        let sources = tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Audio && !t.mix.mute && (!solo || t.mix.solo))
            .flat_map(|t| {
                t.clips.iter().filter_map(move |c| {
                    let item = project.media(c.media()?)?.clone();
                    let stream = match &c.content {
                        koo_core::model::ClipContent::Media { audio_stream, .. } => audio_stream
                            .or_else(|| item.info.as_ref()?.audio.first().map(|a| a.index))
                            .unwrap_or(0),
                        _ => return None,
                    };
                    Some(Source {
                        clip: c.clone(),
                        item,
                        stream,
                        gain: db(c.audio.gain_db + t.mix.volume_db),
                        pan: t.mix.pan.clamp(-1., 1.),
                    })
                })
            })
            .filter(|s| s.clip.end() > from)
            .collect();
        Self {
            media,
            sources,
            voices: HashMap::new(),
            finished: HashSet::new(),
            from,
            rate,
            position: 0,
            scratch: Vec::new(),
        }
    }

    fn time(&self, frame: u64) -> Time {
        self.from + Time((frame as i128 * FLICKS_PER_SECOND as i128 / self.rate as i128) as i64)
    }

    /// Mix the next block into `out` (interleaved stereo).
    fn mix(&mut self, out: &mut [f32]) {
        out.fill(0.);
        let frames = out.len() / 2;
        let (t0, t1) = (
            self.time(self.position),
            self.time(self.position + frames as u64),
        );
        for source in &self.sources {
            let clip = &source.clip;
            let id = clip.id;
            if clip.start >= t1 || clip.end() <= t0 || self.finished.contains(&id) {
                continue;
            }
            // First frame of this block the clip covers.
            let offset = if clip.start > t0 {
                ((clip.start - t0).0 as i128 * self.rate as i128 / FLICKS_PER_SECOND as i128)
                    as usize
            } else {
                0
            }
            .min(frames);
            if !self.voices.contains_key(&id) {
                let at = self.time(self.position + offset as u64).max(clip.start);
                let Some(source_t) = clip.to_source(at) else {
                    continue;
                };
                match self
                    .media
                    .audio(&source.item, source.stream, source_t, self.rate)
                {
                    Ok(voice) => {
                        self.voices.insert(id, voice);
                    }
                    Err(_) => {
                        self.finished.insert(id);
                        continue;
                    }
                }
            }
            let end = if clip.end() < t1 {
                ((clip.end() - t0).0 as i128 * self.rate as i128 / FLICKS_PER_SECOND as i128)
                    as usize
            } else {
                frames
            }
            .min(frames);
            if end <= offset {
                continue;
            }
            let want = (end - offset) * 2;
            self.scratch.resize(want, 0.);
            let voice = self.voices.get_mut(&id).unwrap();
            let mut filled = 0;
            while filled < want {
                match voice.read(&mut self.scratch[filled..want]) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => filled += n,
                }
            }
            self.scratch[filled..want].fill(0.);
            let (pl, pr) = ((1. - source.pan).min(1.), (1. + source.pan).min(1.));
            for i in 0..(end - offset) {
                let t = self.time(self.position + (offset + i) as u64);
                let g = source.gain * fade(clip, t);
                out[(offset + i) * 2] += self.scratch[i * 2] * g * pl;
                out[(offset + i) * 2 + 1] += self.scratch[i * 2 + 1] * g * pr;
            }
            if clip.end() <= t1 {
                self.voices.remove(&id);
                self.finished.insert(id);
            }
        }
        for s in out.iter_mut() {
            *s = s.clamp(-1., 1.);
        }
        self.position += frames as u64;
    }
}

fn db(db: f32) -> f32 {
    10f32.powf(db / 20.)
}

fn fade(clip: &Clip, t: Time) -> f32 {
    let into = (t - clip.start).0 as f32;
    let left = (clip.end() - t).0 as f32;
    let mut g = 1.;
    if clip.audio.fade_in.0 > 0 {
        g *= (into / clip.audio.fade_in.0 as f32).clamp(0., 1.);
    }
    if clip.audio.fade_out.0 > 0 {
        g *= (left / clip.audio.fade_out.0 as f32).clamp(0., 1.);
    }
    g
}
