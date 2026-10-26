//! Development seed behind `KOOEDITOR_DEMO=1`: a small project, fake media
//! and analysis services, and a selection, so every panel can be looked at
//! without real media. `KOOEDITOR_DEMO_WORKSPACE=captions` (or any workspace
//! label) opens that workspace.

use crate::state::EditorState;
use koo_core::agent::Workspace;
use koo_core::analysis::{Analysis, Loudness, Speaker, Transcript, Word};
use koo_core::bus::CommandBus;
use koo_core::model::*;
use koo_core::services::*;
use koo_core::{BinId, JobId, MediaId, Rate, SpeakerId, Time, TimeRange};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub fn enabled() -> bool {
    std::env::var("KOOEDITOR_DEMO").is_ok_and(|v| v == "1")
}

pub fn seed(state: &mut EditorState) {
    let mut project = Project::new("Demo");
    let bins = [
        Bin {
            id: BinId::new(),
            name: "Interviews".into(),
            parent: None,
        },
        Bin {
            id: BinId::new(),
            name: "B-roll".into(),
            parent: None,
        },
        Bin {
            id: BinId::new(),
            name: "Music".into(),
            parent: None,
        },
    ];
    let speakers = [SpeakerId::new(), SpeakerId::new()];

    let interview_a = video(
        "interview_anna.mp4",
        Some(bins[0].id),
        &["interview", "anna"],
        723.0,
        &[Some("en"), Some("es")],
    );
    let interview_b = video(
        "interview_ben.mp4",
        Some(bins[0].id),
        &["interview"],
        512.0,
        &[Some("en")],
    );
    let broll = video(
        "city_night.mov",
        Some(bins[1].id),
        &["b-roll", "night"],
        38.0,
        &[],
    );
    let broll2 = video("harbor_drone.mp4", Some(bins[1].id), &["drone"], 64.0, &[]);
    let music = audio("ambient_bed.wav", Some(bins[2].id), &["music"], 184.0);
    let logo = MediaItem {
        id: MediaId::new(),
        path: PathBuf::from("/demo/logo.png"),
        kind: MediaKind::Image,
        fingerprint: Some(Fingerprint("demo-logo".into())),
        info: None,
        bin: None,
        tags: vec!["brand".into()],
        proxy: None,
    };

    let mut transcripts = HashMap::new();
    transcripts.insert(
        interview_a.fingerprint.clone().unwrap(),
        transcript(&interview_a, ANNA, 8.0, speakers[0], &speakers),
    );
    transcripts.insert(
        interview_b.fingerprint.clone().unwrap(),
        transcript(&interview_b, BEN, 3.0, speakers[1], &speakers),
    );

    let tl = &mut project.timeline;
    let s = Time::from_secs_f64;
    let v1 = tl
        .tracks
        .iter()
        .position(|t| t.kind == TrackKind::Video)
        .unwrap();
    let a1 = tl
        .tracks
        .iter()
        .position(|t| t.kind == TrackKind::Audio)
        .unwrap();
    let place = |media: &MediaItem, start: f64, src: f64, dur: f64, stream: Option<u32>| {
        Clip::new(
            s(start),
            s(dur),
            ClipContent::Media {
                media: media.id,
                source_in: s(src),
                audio_stream: stream,
            },
        )
    };
    let mut first = place(&interview_a, 0.0, 8.0, 34.0, None);
    first.keyframes = vec![
        Keyframe {
            property: Property::Scale,
            at: s(2.0),
            value: 1.0,
            ease: Ease::EaseInOut,
        },
        Keyframe {
            property: Property::Scale,
            at: s(4.5),
            value: 1.35,
            ease: Ease::EaseInOut,
        },
        Keyframe {
            property: Property::PositionX,
            at: s(4.5),
            value: 0.12,
            ease: Ease::Linear,
        },
    ];
    first.grade.exposure = 0.2;
    first.grade.saturation = 1.1;
    let selected = first.id;
    tl.tracks[v1].clips = vec![
        first,
        place(&broll, 34.0, 4.0, 6.0, None),
        place(&interview_b, 40.0, 3.0, 30.0, None),
    ];
    let mut a = place(&interview_a, 0.0, 8.0, 34.0, Some(1));
    a.audio.gain_db = -2.5;
    a.audio.fade_in = Time::from_millis(250);
    tl.tracks[a1].clips = vec![a, place(&interview_b, 40.0, 3.0, 30.0, Some(1))];
    let mut music_track = Track::new(TrackKind::Audio, "A2");
    music_track.mix.volume_db = -9.0;
    let mut bed = place(&music, 0.0, 0.0, 70.0, Some(0));
    bed.audio.fade_out = s(3.0);
    music_track.clips = vec![bed];
    tl.ducking = Some(Ducking {
        music_track: music_track.id,
        reduction_db: -12.0,
        attack: Time::from_millis(200),
        release: Time::from_millis(600),
    });
    tl.tracks.push(music_track);

    let media = vec![interview_a, interview_b, broll, broll2, music, logo];
    let jobs = vec![
        job("Analysis", &media[2], JobState::Running { fraction: 0.42 }),
        job("Analysis", &media[3], JobState::Queued),
        job("Analysis", &media[0], JobState::Done),
        job("Analysis", &media[1], JobState::Done),
    ];
    project.media = media;
    state.bus = CommandBus::new(project);
    state.pool_bins = bins.to_vec();
    state.selection.clips = vec![selected];
    state.playhead = s(5.2);
    state.services = Services {
        media: Some(Arc::new(DemoMedia)),
        analysis: Some(Arc::new(DemoAnalysis {
            transcripts,
            jobs: Mutex::new(jobs),
        })),
    };
    if let Ok(name) = std::env::var("KOOEDITOR_DEMO_WORKSPACE")
        && let Some(w) = Workspace::ALL
            .into_iter()
            .find(|w| w.label().eq_ignore_ascii_case(&name))
    {
        state.workspace = w;
    }
}

fn fingerprint(name: &str) -> Fingerprint {
    Fingerprint(format!(
        "{:016x}{:016x}",
        hash(name),
        hash(&name.len().to_string())
    ))
}

fn hash(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
    })
}

fn video(
    name: &str,
    bin: Option<BinId>,
    tags: &[&str],
    secs: f64,
    langs: &[Option<&str>],
) -> MediaItem {
    MediaItem {
        id: MediaId::new(),
        path: PathBuf::from(format!("/demo/{name}")),
        kind: MediaKind::Video,
        fingerprint: Some(fingerprint(name)),
        info: Some(MediaInfo {
            duration: Time::from_secs_f64(secs),
            video: Some(VideoStream {
                index: 0,
                width: 3840,
                height: 2160,
                rate: Rate {
                    num: 60000,
                    den: 1001,
                },
                codec: "hevc".into(),
            }),
            audio: langs
                .iter()
                .enumerate()
                .map(|(i, l)| AudioStream {
                    index: i as u32 + 1,
                    channels: 2,
                    sample_rate: 48_000,
                    codec: "aac".into(),
                    language: l.map(Into::into),
                })
                .collect(),
            container: "mp4".into(),
        }),
        bin,
        tags: tags.iter().map(|t| t.to_string()).collect(),
        proxy: None,
    }
}

fn audio(name: &str, bin: Option<BinId>, tags: &[&str], secs: f64) -> MediaItem {
    MediaItem {
        id: MediaId::new(),
        path: PathBuf::from(format!("/demo/{name}")),
        kind: MediaKind::Audio,
        fingerprint: Some(fingerprint(name)),
        info: Some(MediaInfo {
            duration: Time::from_secs_f64(secs),
            video: None,
            audio: vec![AudioStream {
                index: 0,
                channels: 2,
                sample_rate: 48_000,
                codec: "pcm_s24le".into(),
                language: None,
            }],
            container: "wav".into(),
        }),
        bin,
        tags: tags.iter().map(|t| t.to_string()).collect(),
        proxy: None,
    }
}

fn job(label: &str, media: &MediaItem, state: JobState) -> JobStatus {
    JobStatus {
        id: JobId::new(),
        label: label.into(),
        media: Some(media.id),
        state,
    }
}

const ANNA: &str = "So the idea started, um, almost by accident. We were filming the harbor at night \
and the lights just kept flickering on the water. I said, what if the whole piece feels like that? \
Quiet, a little unstable, but warm. Honestly, we threw out the first draft completely. It was too \
polished. The second version kept the mistakes, the breathing, the pauses. That's where it came alive.";

const BEN: &str = "Right, and from the sound side we wanted the city to feel close. Every recording \
was done on location. No library effects at all. Sometimes you hear a bus, sometimes a gull, and \
we leave it in. People notice that, even if they can't say why.";

/// Evenly paced words with pauses after punctuation; a few hesitations get
/// low confidence, and the second speaker answers every few sentences.
fn transcript(
    item: &MediaItem,
    text: &str,
    from: f64,
    speaker: SpeakerId,
    speakers: &[SpeakerId; 2],
) -> Analysis {
    let mut t = from;
    let mut words = Vec::new();
    let mut sentence = 0;
    for (i, raw) in text.split_whitespace().enumerate() {
        let len = 0.18 + raw.len() as f64 * 0.035;
        let low = raw.starts_with("um") || i % 23 == 7;
        let who = if speaker == speakers[0] && sentence % 4 == 3 {
            speakers[1]
        } else {
            speaker
        };
        words.push(Word {
            range: TimeRange::new(Time::from_secs_f64(t), Time::from_secs_f64(t + len)),
            text: raw.into(),
            confidence: if low { 0.32 } else { 0.93 },
            speaker: Some(who),
        });
        t += len + 0.06;
        if raw.ends_with(['.', '?', '!']) {
            t += 0.55;
            sentence += 1;
        } else if raw.ends_with(',') {
            t += 0.25;
        }
    }
    Analysis {
        transcript: Some(Transcript {
            fingerprint: item.fingerprint.clone().unwrap(),
            audio_stream: 1,
            language: "en".into(),
            words,
            speakers: vec![
                Speaker {
                    id: speakers[0],
                    label: "Anna".into(),
                },
                Speaker {
                    id: speakers[1],
                    label: "Ben".into(),
                },
            ],
        }),
        speech: Vec::new(),
        shot_changes: Vec::new(),
    }
}

struct DemoAnalysis {
    transcripts: HashMap<Fingerprint, Analysis>,
    jobs: Mutex<Vec<JobStatus>>,
}

impl AnalysisService for DemoAnalysis {
    fn analyze(&self, item: &MediaItem) -> JobId {
        let status = job("Analysis", item, JobState::Queued);
        let id = status.id;
        self.jobs.lock().unwrap().push(status);
        id
    }

    fn jobs(&self) -> Vec<JobStatus> {
        self.jobs.lock().unwrap().clone()
    }

    fn analysis(&self, fingerprint: &Fingerprint) -> Option<Arc<Analysis>> {
        self.transcripts.get(fingerprint).cloned().map(Arc::new)
    }

    fn silences(&self, _: &MediaItem, _: Time) -> ServiceResult<Vec<TimeRange>> {
        Ok(Vec::new())
    }

    fn loudness(&self, path: &Path, _: Option<TimeRange>) -> ServiceResult<Loudness> {
        let h = hash(&path.to_string_lossy());
        Ok(Loudness {
            integrated_lufs: -24.0 + (h % 90) as f32 / 10.0,
            true_peak_db: -6.0 + (h % 50) as f32 / 10.0,
            range_lu: 6.5,
        })
    }
}

/// Gradient frames tinted per file, so thumbnails look distinct.
struct DemoMedia;

impl MediaService for DemoMedia {
    fn probe(&self, _: &Path) -> ServiceResult<MediaInfo> {
        Err(ServiceError::new("demo"))
    }

    fn fingerprint(&self, path: &Path) -> ServiceResult<Fingerprint> {
        Ok(fingerprint(&path.to_string_lossy()))
    }

    fn frame(&self, item: &MediaItem, _: Time, max_width: u32) -> ServiceResult<Frame> {
        let (w, h) = (max_width.min(320), max_width.min(320) * 9 / 16);
        let seed = hash(&item.path.to_string_lossy());
        let base = [
            (seed & 0xff) as f32,
            (seed >> 8 & 0xff) as f32,
            (seed >> 16 & 0xff) as f32,
        ];
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let fx = x as f32 / w as f32;
                let fy = y as f32 / h as f32;
                let glow = (1.0 - ((fx - 0.65).powi(2) + (fy - 0.4).powi(2)).sqrt() * 1.6).max(0.0);
                for c in base {
                    let v = 18.0 + c * 0.35 * (1.0 - fy * 0.6) + glow * c * 0.5;
                    rgba.push(v.min(255.0) as u8);
                }
                rgba.push(255);
            }
        }
        Ok(Frame {
            width: w,
            height: h,
            rgba: rgba.into(),
        })
    }

    fn playback(&self, _: &MediaItem, _: Time, _: u32) -> ServiceResult<Box<dyn FrameStream>> {
        Err(ServiceError::new("demo"))
    }

    fn audio(&self, _: &MediaItem, _: u32, _: Time, _: u32) -> ServiceResult<Box<dyn PcmStream>> {
        Err(ServiceError::new("demo"))
    }

    fn collage(&self, _: &CollageRequest) -> ServiceResult<Collage> {
        Err(ServiceError::new("demo"))
    }

    fn waveform(&self, _: &MediaItem, _: u32, _: TimeRange, n: usize) -> ServiceResult<Vec<f32>> {
        Ok((0..n)
            .map(|i| (i as f32 * 0.37).sin().abs() * 0.8)
            .collect())
    }

    fn ensure_proxy(&self, _: &MediaItem, _: &dyn Fn(f32)) -> ServiceResult<PathBuf> {
        Err(ServiceError::new("demo"))
    }
}
