//! Fake engines for demo mode: procedural frames, waveforms and audio for
//! files that don't exist, plus canned analysis.

use koo_core::analysis::{Analysis, Loudness};
use koo_core::model::*;
use koo_core::services::*;
use koo_core::{JobId, Rate, Time, TimeRange};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

fn hash(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn unit(x: u64) -> f64 {
    (hash(x) >> 11) as f64 / (1u64 << 53) as f64
}

fn seed_of(item: &MediaItem) -> u64 {
    item.id.0.as_u64_pair().0
}

fn is_music(item: &MediaItem) -> bool {
    item.kind == MediaKind::Audio
}

/// Speech phrases of a fake source, in seconds.
fn phrases(item: &MediaItem) -> Vec<(f64, f64)> {
    let len = item
        .info
        .as_ref()
        .map(|i| i.duration.as_secs_f64())
        .unwrap_or(60.);
    if is_music(item) {
        return vec![(0., len)];
    }
    let seed = seed_of(item);
    let (mut t, mut k, mut out) = (0.2, 0u64, Vec::new());
    while t < len {
        let speech = 1.5 + 4.5 * unit(seed ^ (k * 2));
        let pause = 0.15 + 1.4 * unit(seed ^ (k * 2 + 1)).powi(2);
        out.push((t, (t + speech).min(len)));
        t += speech + pause;
        k += 1;
    }
    out
}

/// 0..1 loudness of a fake source at `t` seconds.
fn level(item: &MediaItem, phrases: &[(f64, f64)], t: f64) -> f32 {
    let seed = seed_of(item);
    if is_music(item) {
        let beat = (t * 2.0 * std::f64::consts::PI).sin().abs();
        return (0.35 + 0.25 * beat + 0.1 * unit(seed ^ (t * 40.) as u64)) as f32;
    }
    let ix = phrases.partition_point(|p| p.1 <= t);
    match phrases.get(ix) {
        Some(&(a, b)) if t >= a => {
            let syllable = (t * 7.0 + unit(seed ^ ix as u64) * 6.).sin().abs();
            let edge = ((t - a).min(b - t) * 8.).min(1.);
            (edge * (0.25 + 0.6 * syllable * (0.6 + 0.4 * unit(seed ^ (t * 25.) as u64)))) as f32
        }
        _ => (0.02 * unit(seed ^ (t * 50.) as u64)) as f32,
    }
}

fn fake_frame(item: &MediaItem, t: Time, max_width: u32) -> Frame {
    let width = max_width.clamp(64, 960);
    let height = width * 9 / 16;
    let seed = seed_of(item);
    let hue = unit(seed) as f32;
    let secs = t.as_secs_f64() as f32;
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    let sun_x = 0.3 + 0.4 * ((secs * 0.15).sin() * 0.5 + 0.5);
    for y in 0..height {
        let v = y as f32 / height as f32;
        for x in 0..width {
            let u = x as f32 / width as f32;
            let horizon = 0.62 + 0.03 * (u * 9. + hue * 20.).sin();
            let (h, s, l) = if v < horizon {
                let d = ((u - sun_x).powi(2) + (v - 0.35).powi(2)).sqrt();
                let glow = (1. - d * 3.).max(0.);
                (hue, 0.45, 0.22 + 0.35 * (1. - v) + 0.4 * glow)
            } else {
                ((hue + 0.35) % 1., 0.3, 0.12 + 0.1 * (1. - v))
            };
            let (r, g, b) = hsl_to_rgb(h, s, l.min(0.95));
            let i = ((y * width + x) * 4) as usize;
            rgba[i..i + 4].copy_from_slice(&[r, g, b, 255]);
        }
    }
    Frame {
        width,
        height,
        rgba: rgba.into(),
    }
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let c = (1. - (2. * l - 1.).abs()) * s;
    let hp = h * 6.;
    let x = c * (1. - (hp % 2. - 1.).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.),
        1 => (x, c, 0.),
        2 => (0., c, x),
        3 => (0., x, c),
        4 => (x, 0., c),
        _ => (c, 0., x),
    };
    let m = l - c / 2.;
    let to = |v: f32| ((v + m).clamp(0., 1.) * 255.) as u8;
    (to(r), to(g), to(b))
}

pub struct FakeMedia;

struct FakeFrames {
    item: MediaItem,
    t: Time,
    max_width: u32,
}

impl FrameStream for FakeFrames {
    fn next_frame(&mut self) -> ServiceResult<Option<(Time, Frame)>> {
        let t = self.t;
        self.t = t + Rate::FPS_30.frame_duration();
        Ok(Some((t, fake_frame(&self.item, t, self.max_width))))
    }
}

struct FakePcm {
    item: MediaItem,
    phrases: Vec<(f64, f64)>,
    sample: u64,
    start: f64,
    rate: u32,
}

impl PcmStream for FakePcm {
    fn read(&mut self, out: &mut [f32]) -> ServiceResult<usize> {
        let freq = if is_music(&self.item) { 110. } else { 190. };
        for pair in out.chunks_mut(2) {
            let t = self.start + self.sample as f64 / self.rate as f64;
            let env = level(&self.item, &self.phrases, t) as f64;
            let v = (t * freq * std::f64::consts::TAU).sin() * env * 0.15;
            pair.fill(v as f32);
            self.sample += 1;
        }
        Ok(out.len())
    }
}

impl MediaService for FakeMedia {
    fn probe(&self, _: &Path) -> ServiceResult<MediaInfo> {
        Err(ServiceError::new("demo media cannot probe"))
    }

    fn fingerprint(&self, _: &Path) -> ServiceResult<Fingerprint> {
        Err(ServiceError::new("demo media cannot fingerprint"))
    }

    fn frame(&self, item: &MediaItem, t: Time, max_width: u32) -> ServiceResult<Frame> {
        Ok(fake_frame(item, t, max_width))
    }

    fn playback(
        &self,
        item: &MediaItem,
        from: Time,
        max_width: u32,
    ) -> ServiceResult<Box<dyn FrameStream>> {
        Ok(Box::new(FakeFrames {
            item: item.clone(),
            t: from.floor_to_frame(Rate::FPS_30),
            max_width: max_width.min(640),
        }))
    }

    fn audio(
        &self,
        item: &MediaItem,
        _stream: u32,
        from: Time,
        sample_rate: u32,
    ) -> ServiceResult<Box<dyn PcmStream>> {
        Ok(Box::new(FakePcm {
            item: item.clone(),
            phrases: phrases(item),
            sample: 0,
            start: from.as_secs_f64(),
            rate: sample_rate,
        }))
    }

    fn collage(&self, _: &CollageRequest) -> ServiceResult<Collage> {
        Err(ServiceError::new("demo media has no collages"))
    }

    fn waveform(
        &self,
        item: &MediaItem,
        _stream: u32,
        range: TimeRange,
        buckets: usize,
    ) -> ServiceResult<Vec<f32>> {
        let phrases = phrases(item);
        let (a, len) = (range.start.as_secs_f64(), range.duration().as_secs_f64());
        Ok((0..buckets)
            .map(|i| {
                let t0 = a + len * i as f64 / buckets as f64;
                let t1 = a + len * (i + 1) as f64 / buckets as f64;
                (0..4)
                    .map(|k| level(item, &phrases, t0 + (t1 - t0) * k as f64 / 4.))
                    .fold(0., f32::max)
            })
            .collect())
    }

    fn ensure_proxy(&self, _: &MediaItem, _: &dyn Fn(f32)) -> ServiceResult<PathBuf> {
        Err(ServiceError::new("demo media has no proxies"))
    }
}

pub struct FakeAnalysis {
    pub analyses: HashMap<Fingerprint, Arc<Analysis>>,
    pub jobs: Mutex<Vec<JobStatus>>,
}

impl AnalysisService for FakeAnalysis {
    fn analyze(&self, item: &MediaItem) -> JobId {
        let id = JobId::new();
        self.jobs.lock().unwrap().push(JobStatus {
            id,
            label: "Analysis".into(),
            media: Some(item.id),
            state: JobState::Queued,
        });
        id
    }

    fn jobs(&self) -> Vec<JobStatus> {
        self.jobs.lock().unwrap().clone()
    }

    fn analysis(&self, fingerprint: &Fingerprint) -> Option<Arc<Analysis>> {
        self.analyses.get(fingerprint).cloned()
    }

    fn silences(&self, item: &MediaItem, min: Time) -> ServiceResult<Vec<TimeRange>> {
        let len = item.info.as_ref().map(|i| i.duration).unwrap_or(Time::ZERO);
        if is_music(item) {
            // No speech anywhere.
            return Ok(vec![TimeRange::new(Time::ZERO, len)]);
        }
        // Gaps between transcribed words when there is a transcript, else
        // between the procedural phrases.
        let speech: Vec<TimeRange> =
            match item.fingerprint.as_ref().and_then(|f| self.analyses.get(f)) {
                Some(a) => a.speech.clone(),
                None => phrases(item).into_iter().map(|(a, b)| secs(a, b)).collect(),
            };
        Ok(speech
            .windows(2)
            .map(|w| TimeRange::new(w[0].end, w[1].start))
            .filter(|gap| gap.duration() >= min)
            .collect())
    }

    fn loudness(&self, path: &Path, _: Option<TimeRange>) -> ServiceResult<Loudness> {
        let h = hash(path.to_string_lossy().len() as u64);
        Ok(Loudness {
            integrated_lufs: -24.0 + (h % 90) as f32 / 10.0,
            true_peak_db: -6.0 + (h % 50) as f32 / 10.0,
            range_lu: 6.5,
        })
    }
}

fn secs(a: f64, b: f64) -> TimeRange {
    TimeRange::new(Time::from_secs_f64(a), Time::from_secs_f64(b))
}
