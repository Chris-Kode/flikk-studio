//! Development seed data, behind `KOOEDITOR_DEMO=1`: a small interview edit
//! with captions, titles, markers and a pending agent change set, plus fake
//! media and analysis engines, so the timeline and viewer can be exercised
//! and screenshotted without real media.

use crate::state::EditorState;
use gpui_kit::*;
use koo_core::analysis::{Analysis, Loudness};
use koo_core::bus::{AgentRole, Author, CommandBus};
use koo_core::command::Command;
use koo_core::model::{Transform, *};
use koo_core::services::*;
use koo_core::{JobId, MediaId, Rate, Time, TimeRange};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub fn enabled() -> bool {
    std::env::var("KOOEDITOR_DEMO").is_ok_and(|v| v == "1")
}

pub fn seed(state: &mut EditorState, cx: &mut Context<EditorState>) {
    let (project, ids) = project();
    let mut bus = CommandBus::new(project);
    bus.set_permissions(state.bus.permissions().clone());
    let agent = Author::Agent {
        role: AgentRole::Editor,
    };
    let proposals = [
        (
            Command::DeleteClips {
                clips: vec![ids.street],
                ripple: false,
            },
            "Remove the street shot; it repeats 0:08",
        ),
        (
            Command::ApplyTitle {
                track: ids.titles,
                template: "lower-third".into(),
                range: secs(12.0, 16.0),
                text: "Maya Chen · Director".into(),
            },
            "Lower third when Maya first speaks",
        ),
        (
            Command::SetTransform {
                clip: ids.punch,
                transform: Transform {
                    scale: 1.15,
                    ..Transform::default()
                },
            },
            "Punch in to 115% on the key line",
        ),
        (
            Command::Blade {
                at: Time::from_secs_f64(28.0),
                tracks: Some(vec![ids.music]),
            },
            "Split the music at the scene change",
        ),
        (
            Command::RippleDelete {
                range: secs(46.2, 47.0),
                tracks: None,
            },
            "Tighten a 0.8 s pause",
        ),
    ];
    let mut set = None;
    for (command, reason) in proposals {
        if let Ok(koo_core::bus::Outcome::Proposed { change_set, .. }) =
            bus.execute(command, agent.clone(), reason)
        {
            set = Some(change_set);
        }
    }
    if let Some(set) = set {
        bus.close_change_set(set);
    }
    state.bus = bus;
    state.playhead = Time::from_secs_f64(2.5);
    state.selection.clips = vec![ids.selected];
    // The demo media are not real files: always use the fake engines.
    state.services.media = Some(Arc::new(FakeMedia));
    state.services.analysis = Some(Arc::new(FakeAnalysis));
    cx.notify();
}

struct Ids {
    titles: koo_core::TrackId,
    music: koo_core::TrackId,
    street: koo_core::ClipId,
    punch: koo_core::ClipId,
    selected: koo_core::ClipId,
}

fn secs(a: f64, b: f64) -> TimeRange {
    TimeRange::new(Time::from_secs_f64(a), Time::from_secs_f64(b))
}

fn media(name: &str, kind: MediaKind, duration: f64) -> MediaItem {
    MediaItem {
        id: MediaId::new(),
        path: PathBuf::from(format!("/demo/{name}")),
        kind,
        fingerprint: None,
        info: Some(MediaInfo {
            duration: Time::from_secs_f64(duration),
            video: (kind == MediaKind::Video).then(|| VideoStream {
                index: 0,
                width: 1920,
                height: 1080,
                rate: Rate::FPS_30,
                codec: "h264".into(),
            }),
            audio: vec![AudioStream {
                index: 1,
                channels: 2,
                sample_rate: 48_000,
                codec: "aac".into(),
                language: Some("en".into()),
            }],
            container: "mp4".into(),
        }),
        bin: None,
        tags: vec![],
        proxy: None,
    }
}

fn media_clip(item: &MediaItem, at: f64, source: f64, len: f64, audio: bool) -> Clip {
    Clip::new(
        Time::from_secs_f64(at),
        Time::from_secs_f64(len),
        ClipContent::Media {
            media: item.id,
            source_in: Time::from_secs_f64(source),
            audio_stream: audio.then_some(1),
        },
    )
}

fn project() -> (Project, Ids) {
    let mut p = Project::new("Demo");
    let interview = media("interview_maya.mp4", MediaKind::Video, 300.);
    let city = media("broll_city.mp4", MediaKind::Video, 40.);
    let street = media("broll_street.mp4", MediaKind::Video, 40.);
    let music = media("music_bed.wav", MediaKind::Audio, 180.);

    let mut titles = Track::new(TrackKind::Title, "T1");
    titles.clips.push(Clip::new(
        Time::from_secs_f64(0.4),
        Time::from_secs_f64(4.2),
        ClipContent::Title {
            template: "center".into(),
            text: "The Long Way Home".into(),
        },
    ));

    let mut captions = Track::new(TrackKind::Caption, "C1");
    let lines = [
        (0.6, 3.4, "I never planned to make films."),
        (3.6, 6.8, "I was an engineer, basically."),
        (7.0, 10.6, "Then my father handed me his camera."),
        (11.4, 14.2, "That changed everything."),
        (14.5, 18.0, "We drove north for three weeks."),
        (18.3, 22.9, "No script, no crew, just the two of us."),
        (24.4, 28.6, "Some days we filmed nothing at all."),
        (29.0, 33.1, "But the quiet days were the real story."),
        (34.0, 37.2, "He talked about his own father."),
        (37.8, 41.5, "Things he had never told anyone."),
        (42.0, 45.6, "I kept the camera rolling."),
        (46.4, 50.5, "This film is for him."),
        (52.4, 56.0, "And for everyone who stayed quiet."),
        (56.6, 61.2, "Sometimes the long way is the only way."),
    ];
    for (a, b, text) in lines {
        captions.clips.push(Clip::new(
            Time::from_secs_f64(a),
            Time::from_secs_f64(b - a),
            ClipContent::Caption {
                text: text.into(),
                speaker: None,
            },
        ));
    }

    let mut broll = Track::new(TrackKind::Video, "V2");
    broll.clips.push(media_clip(&city, 7.5, 2.0, 5.0, false));
    let street_clip = media_clip(&street, 30.0, 14.0, 4.5, false);
    let street_id = street_clip.id;
    broll.clips.push(street_clip);

    let mut main = Track::new(TrackKind::Video, "V1");
    let mut dialog = Track::new(TrackKind::Audio, "A1");
    let segments = [
        (0.0, 3.0, 11.2),
        (11.2, 18.4, 12.8),
        (24.0, 40.0, 13.5),
        (37.5, 61.2, 14.5),
        (52.0, 80.0, 12.0),
    ];
    for (at, source, len) in segments {
        main.clips
            .push(media_clip(&interview, at, source, len, false));
        dialog
            .clips
            .push(media_clip(&interview, at, source, len, true));
    }
    let punch = main.clips[2].id;
    let selected = main.clips[1].id;

    let mut bed = Track::new(TrackKind::Audio, "A2");
    let mut music_clip = media_clip(&music, 0.0, 0.0, 64.0, true);
    music_clip.audio.gain_db = -18.;
    music_clip.audio.fade_in = Time::from_secs_f64(2.);
    music_clip.audio.fade_out = Time::from_secs_f64(3.);
    bed.clips.push(music_clip);

    let ids = Ids {
        titles: titles.id,
        music: bed.id,
        street: street_id,
        punch,
        selected,
    };
    p.media = vec![interview, city, street, music];
    p.timeline.tracks = vec![titles, captions, broll, main, dialog, bed];
    p.timeline.settings.rate = Rate::FPS_30;
    p.timeline.markers = vec![
        Marker {
            id: koo_core::MarkerId::new(),
            range: secs(19.0, 19.0),
            kind: MarkerKind::Note,
            note: "Great quote".into(),
        },
        Marker {
            id: koo_core::MarkerId::new(),
            range: secs(44.0, 49.0),
            kind: MarkerKind::Protected,
            note: "Sponsor read".into(),
        },
    ];
    (p, ids)
}

// ------------------------------------------------------------- fake engines

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

struct FakeMedia;

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

struct FakeAnalysis;

impl AnalysisService for FakeAnalysis {
    fn analyze(&self, _: &MediaItem) -> JobId {
        JobId::new()
    }

    fn jobs(&self) -> Vec<JobStatus> {
        Vec::new()
    }

    fn analysis(&self, _: &Fingerprint) -> Option<Arc<Analysis>> {
        None
    }

    fn silences(&self, item: &MediaItem, min: Time) -> ServiceResult<Vec<TimeRange>> {
        let min = min.as_secs_f64();
        if is_music(item) {
            // No speech anywhere.
            let len = item.info.as_ref().map(|i| i.duration).unwrap_or(Time::ZERO);
            return Ok(vec![TimeRange::new(Time::ZERO, len)]);
        }
        Ok(phrases(item)
            .windows(2)
            .filter(|w| w[1].0 - w[0].1 >= min)
            .map(|w| secs(w[0].1, w[1].0))
            .collect())
    }

    fn loudness(&self, _: &Path, _: Option<TimeRange>) -> ServiceResult<Loudness> {
        Err(ServiceError::new("demo analysis has no loudness"))
    }
}
