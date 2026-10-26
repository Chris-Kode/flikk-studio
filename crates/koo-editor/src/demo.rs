//! Development seed behind `KOOEDITOR_DEMO=1`: one small interview edit
//! with media, bins, captions, a transcript, markers, an agent conversation
//! with an accepted and a pending change set, and fake engines, so every
//! panel can be exercised and screenshotted without real media.
//! `KOOEDITOR_DEMO_WORKSPACE=<label>` opens that workspace.

mod engines;

use crate::state::EditorState;
use engines::{FakeAnalysis, FakeMedia};
use gpui_kit::*;
use koo_core::agent::*;
use koo_core::analysis::{Analysis, Speaker, Transcript, Word};
use koo_core::bus::{AgentRole, Author, CommandBus, Outcome};
use koo_core::command::Command;
use koo_core::model::*;
use koo_core::services::{JobState, JobStatus};
use koo_core::*;
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub fn enabled() -> bool {
    std::env::var("KOOEDITOR_DEMO").is_ok_and(|v| v == "1")
}

pub fn seed(state: &mut EditorState, cx: &mut Context<EditorState>) {
    let demo = project();
    let analysis = interview_analysis(&demo);
    let jobs = vec![
        job(&demo.project.media[0], JobState::Done),
        job(&demo.project.media[1], JobState::Running { fraction: 0.42 }),
        job(&demo.project.media[2], JobState::Queued),
    ];

    let mut bus = CommandBus::new(demo.project.clone());
    bus.set_permissions(state.settings.permissions.clone());
    let chat = agent_history(&mut bus, &demo);

    state.bus = bus;
    state.chat = chat;
    state.playhead = Time::from_secs_f64(2.5);
    state.selection.clips = vec![demo.ids.selected];
    // The demo media are not real files: always use the fake engines.
    state.services.media = Some(Arc::new(FakeMedia));
    state.services.analysis = Some(Arc::new(FakeAnalysis {
        analyses: HashMap::from([(analysis.0, Arc::new(analysis.1))]),
        jobs: Mutex::new(jobs),
    }));
    if let Ok(name) = std::env::var("KOOEDITOR_DEMO_WORKSPACE")
        && let Some(w) = Workspace::ALL
            .into_iter()
            .find(|w| w.label().eq_ignore_ascii_case(&name))
    {
        state.workspace = w;
    }
    chat_references(state, &demo, cx);
    cx.notify();
}

// ------------------------------------------------------------------ project

struct Ids {
    titles: TrackId,
    main: TrackId,
    street: ClipId,
    pointing: [ClipId; 2],
    music: ClipId,
    selected: ClipId,
}

struct Demo {
    project: Project,
    ids: Ids,
    /// Caption lines in timeline seconds; the transcript is built from them.
    lines: Vec<(f64, f64, &'static str)>,
}

const LINES: [(f64, f64, &str); 14] = [
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

fn project() -> Demo {
    let mut p = Project::new("The Long Way Home");
    let bins = [bin("Interviews"), bin("B-roll"), bin("Music")];
    let interview = media(
        "interview_maya.mp4",
        MediaKind::Video,
        300.,
        &bins[0],
        &["interview", "maya"],
    );
    let city = media(
        "broll_city.mp4",
        MediaKind::Video,
        40.,
        &bins[1],
        &["b-roll", "night"],
    );
    let street = media(
        "broll_street.mp4",
        MediaKind::Video,
        40.,
        &bins[1],
        &["b-roll"],
    );
    let music = media(
        "music_bed.wav",
        MediaKind::Audio,
        180.,
        &bins[2],
        &["music"],
    );
    let logo = MediaItem {
        id: MediaId::new(),
        path: PathBuf::from("/demo/logo.png"),
        kind: MediaKind::Image,
        fingerprint: Some(fingerprint("logo.png")),
        info: None,
        bin: None,
        tags: vec!["brand".into()],
        proxy: None,
    };

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
    for (a, b, text) in LINES {
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
    let street_clip = media_clip(&street, 62.0, 14.0, 4.5, false);
    let street_id = street_clip.id;
    broll.clips.push(street_clip);

    let mut main = Track::new(TrackKind::Video, "V1");
    let mut dialog = Track::new(TrackKind::Audio, "A1");
    for (at, source, len) in [
        (0.0, 3.0, 11.2),
        (11.2, 18.4, 12.8),
        (24.0, 40.0, 13.5),
        (37.5, 61.2, 14.5),
        (52.0, 80.0, 12.0),
    ] {
        main.clips
            .push(media_clip(&interview, at, source, len, false));
        dialog
            .clips
            .push(media_clip(&interview, at, source, len, true));
    }
    // A graded opening shot with a gentle push-in, for the inspector.
    let first = &mut main.clips[0];
    first.grade.exposure = 0.2;
    first.grade.saturation = 1.1;
    first.keyframes = vec![
        keyframe(Property::Scale, 2.0, 1.0),
        keyframe(Property::Scale, 4.5, 1.15),
    ];

    let mut bed = Track::new(TrackKind::Audio, "A2");
    bed.mix.volume_db = -9.0;
    let mut music_clip = media_clip(&music, 0.0, 0.0, 64.0, true);
    music_clip.audio.fade_in = Time::from_secs_f64(2.);
    music_clip.audio.fade_out = Time::from_secs_f64(3.);
    let music_id = music_clip.id;
    bed.clips.push(music_clip);

    let ids = Ids {
        titles: titles.id,
        main: main.id,
        street: street_id,
        pointing: [main.clips[1].id, main.clips[2].id],
        music: music_id,
        selected: main.clips[0].id,
    };
    p.timeline.ducking = Some(Ducking {
        music_track: bed.id,
        reduction_db: -12.0,
        attack: Time::from_millis(200),
        release: Time::from_millis(600),
    });
    p.media = vec![interview, city, street, music, logo];
    p.bins = bins.to_vec();
    p.timeline.tracks = vec![titles, captions, broll, main, dialog, bed];
    p.timeline.settings.rate = Rate::FPS_30;
    p.timeline.markers = vec![
        Marker {
            id: MarkerId::new(),
            range: secs(19.0, 19.0),
            kind: MarkerKind::Note,
            note: "Great quote".into(),
        },
        Marker {
            id: MarkerId::new(),
            range: secs(44.0, 49.0),
            kind: MarkerKind::Protected,
            note: "Sponsor read".into(),
        },
    ];
    Demo {
        project: p,
        ids,
        lines: LINES.to_vec(),
    }
}

/// Word-level transcript of the interview in source time, matching the
/// caption lines: each line's words are spread over it and mapped through
/// the V1 clip underneath. A few words get low confidence.
fn interview_analysis(demo: &Demo) -> (Fingerprint, Analysis) {
    let interview = &demo.project.media[0];
    let main = demo
        .project
        .timeline
        .track(demo.ids.main)
        .expect("demo has V1");
    let maya = Speaker {
        id: SpeakerId::new(),
        label: "Maya".into(),
    };
    let mut words = Vec::new();
    for (a, b, text) in &demo.lines {
        let Some(clip) = main.clip_at(Time::from_secs_f64((a + b) / 2.)) else {
            continue;
        };
        let parts: Vec<&str> = text.split_whitespace().collect();
        let step = (b - a) / parts.len() as f64;
        for (i, w) in parts.iter().enumerate() {
            let t0 = Time::from_secs_f64(a + step * i as f64);
            let t1 = Time::from_secs_f64(a + step * (i as f64 + 0.85));
            let (Some(s0), Some(s1)) = (clip.to_source(t0), clip.to_source(t1)) else {
                continue;
            };
            words.push(Word {
                range: TimeRange::new(s0, s1),
                text: (*w).into(),
                confidence: if (words.len() % 17) == 9 { 0.34 } else { 0.94 },
                speaker: Some(maya.id),
            });
        }
    }
    words.sort_by_key(|w| w.range.start);
    let speech = merge_speech(&words);
    let fingerprint = interview
        .fingerprint
        .clone()
        .expect("demo media have fingerprints");
    let analysis = Analysis {
        transcript: Some(Transcript {
            fingerprint: fingerprint.clone(),
            audio_stream: 0,
            language: "en".into(),
            words,
            speakers: vec![maya],
        }),
        speech,
        shot_changes: Vec::new(),
    };
    (fingerprint, analysis)
}

/// Speech regions: words closer than 0.3 s join one region.
fn merge_speech(words: &[Word]) -> Vec<TimeRange> {
    let mut out: Vec<TimeRange> = Vec::new();
    for w in words {
        match out.last_mut() {
            Some(r) if w.range.start - r.end < Time::from_millis(300) => {
                r.end = r.end.max(w.range.end)
            }
            _ => out.push(w.range),
        }
    }
    out
}

// ------------------------------------------------------------ agent history

/// The PRD's example turn, played through the real bus: an accepted lower
/// third, then a pending set of pause cuts and zooms, and a question.
fn agent_history(bus: &mut CommandBus, demo: &Demo) -> Vec<ChatMessage> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let agent = Author::Agent {
        role: AgentRole::Editor,
    };
    let ids = &demo.ids;
    let mut chat = vec![msg(
        ChatRole::User,
        "Put a lower third on Maya when she first speaks",
        now - 900,
    )];

    let proposed = |outcome: koo_core::Result<Outcome>| match outcome {
        Ok(Outcome::Proposed { change_set, .. }) => Some(change_set),
        _ => None,
    };
    if let Some(set) = proposed(bus.execute(
        Command::ApplyTitle {
            track: ids.titles,
            template: "lower-third".into(),
            range: secs(11.4, 15.4),
            text: "Maya Chen · Director".into(),
        },
        agent.clone(),
        "Lower third when Maya first speaks",
    )) {
        bus.close_change_set(set);
        let _ = bus.accept(set, None);
        chat.push(msg(
            ChatRole::Agent,
            "Added a lower third on **T1** at 0:11, in your default title style.",
            now - 880,
        ));
        chat.push(card(set, now - 880));
    }

    chat.push(msg(
        ChatRole::User,
        "remove the long pauses where nothing happens, and zoom in whenever I point something out",
        now - 420,
    ));
    // Zooms first, then cuts latest-first, so every time is still in the
    // original timeline. The first reason titles the set.
    let mut pending = None;
    let items = [
        (
            Command::AddZoom {
                clip: ids.pointing[0],
                range: secs(15.0, 18.0),
                target: [0.25, -0.1],
                scale: 1.8,
                ease: Ease::EaseInOut,
            },
            "Remove 3 long pauses and add 2 zooms",
        ),
        (
            Command::AddZoom {
                clip: ids.pointing[1],
                range: secs(30.0, 33.0),
                target: [-0.2, 0.05],
                scale: 1.8,
                ease: Ease::EaseInOut,
            },
            "Zoom 1.8× where Maya points at the map",
        ),
        (
            Command::SetGain {
                clip: ids.music,
                db: -6.0,
            },
            "Music −6 dB under the quiet-days line",
        ),
        (
            Command::DeleteClips {
                clips: vec![ids.street],
                ripple: false,
            },
            "Remove the street shot; it repeats 0:08",
        ),
        (
            Command::RippleDelete {
                range: secs(50.5, 52.4),
                tracks: None,
            },
            "Pause 1.9 s after “This film is for him.”",
        ),
        (
            Command::RippleDelete {
                range: secs(45.6, 46.4),
                tracks: None,
            },
            "Pause 0.8 s inside the sponsor read",
        ),
        (
            Command::RippleDelete {
                range: secs(22.9, 24.4),
                tracks: None,
            },
            "Pause 1.5 s before the quiet-days line",
        ),
    ];
    for (command, reason) in items {
        pending = proposed(bus.execute(command, agent.clone(), reason)).or(pending);
    }
    chat.push(ChatMessage {
        tools: tools(now),
        ..msg(ChatRole::Agent, "", now - 402)
    });
    if let Some(set) = pending {
        bus.close_change_set(set);
        chat.push(msg(
            ChatRole::Agent,
            "Found **3 pauses** (4.2 s) and **2 moments** where you point at something. \
             Proposed 3 ripple deletes and 2 zooms at 1.8× with ease in/out. \
             One pause sits inside the protected sponsor read.",
            now - 400,
        ));
        chat.push(card(set, now - 400));
    }
    chat.push(ChatMessage {
        choices: vec![
            "Match the last video".into(),
            "Default style".into(),
            "Skip captions".into(),
        ],
        ..msg(
            ChatRole::Agent,
            "Should the captions match your last video's style?",
            now - 395,
        )
    });
    chat
}

fn tools(now: i64) -> Vec<ActivityEntry> {
    let entry = |at: i64, tool: &str, args, summary: &str| ActivityEntry {
        at,
        tool: tool.into(),
        args,
        summary: summary.into(),
        images: vec![],
        excerpts: vec![],
        ok: true,
    };
    vec![
        entry(
            now - 418,
            "get_timeline",
            json!({ "start": 0.0, "end": 80.0 }),
            "6 tracks, 21 clips, 2 markers",
        ),
        entry(
            now - 416,
            "find_silences",
            json!({ "min_s": 0.8 }),
            "3 pauses ≥ 0.8 s, 4.2 s total",
        ),
        ActivityEntry {
            excerpts: vec![
                "0:15  “We drove north for three weeks.”".into(),
                "0:30  “But the quiet days were the real story.”".into(),
            ],
            ..entry(
                now - 414,
                "search_transcript",
                json!({ "text": "north | quiet days" }),
                "2 pointing moments",
            )
        },
        entry(
            now - 410,
            "view_range",
            json!({ "start": "0:14.000", "end": "0:19.000" }),
            "30-frame collage, hand on the map at 0:15.4",
        ),
        entry(
            now - 406,
            "add_zoom",
            json!({ "clip": "v1", "start": 15.0, "end": 18.0, "scale": 1.8 }),
            "Proposed",
        ),
        entry(
            now - 404,
            "ripple_delete",
            json!({ "start": "0:50.500", "end": "0:52.400" }),
            "Proposed, 3 cuts",
        ),
        entry(
            now - 402,
            "set_gain",
            json!({ "clip": "a2", "db": -6.0 }),
            "Proposed",
        ),
    ]
}

// ------------------------------------------------------------------ helpers

fn msg(role: ChatRole, text: &str, at: i64) -> ChatMessage {
    ChatMessage {
        role,
        text: text.into(),
        choices: vec![],
        change_set: None,
        tools: vec![],
        at,
    }
}

fn card(set: ChangeSetId, at: i64) -> ChatMessage {
    ChatMessage {
        change_set: Some(set),
        ..msg(ChatRole::Agent, "", at)
    }
}

fn secs(a: f64, b: f64) -> TimeRange {
    TimeRange::new(Time::from_secs_f64(a), Time::from_secs_f64(b))
}

fn bin(name: &str) -> Bin {
    Bin {
        id: BinId::new(),
        name: name.into(),
        parent: None,
    }
}

fn keyframe(property: Property, at: f64, value: f32) -> Keyframe {
    Keyframe {
        property,
        at: Time::from_secs_f64(at),
        value,
        ease: Ease::EaseInOut,
    }
}

fn fingerprint(name: &str) -> Fingerprint {
    let h = name.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
    });
    Fingerprint(format!("{h:016x}{:016x}", h.rotate_left(17)))
}

fn media(name: &str, kind: MediaKind, duration: f64, bin: &Bin, tags: &[&str]) -> MediaItem {
    MediaItem {
        id: MediaId::new(),
        path: PathBuf::from(format!("/demo/{name}")),
        kind,
        fingerprint: Some(fingerprint(name)),
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
        bin: Some(bin.id),
        tags: tags.iter().map(|t| t.to_string()).collect(),
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

fn job(item: &MediaItem, state: JobState) -> JobStatus {
    JobStatus {
        id: JobId::new(),
        label: "Analysis".into(),
        media: Some(item.id),
        state,
    }
}

// --------------------------------------------------------- chat references

/// `KOOEDITOR_DEMO_CHAT=1`: an effect and a transition on the selected clip,
/// a compressor on the music, a sent message with a reference of every
/// kind, and chips of several kinds waiting in the composer.
fn chat_references(state: &mut EditorState, demo: &Demo, cx: &mut Context<EditorState>) {
    use koo_core::audiofx::{AudioFx, AudioFxKind};
    use koo_core::command::Edge;
    use koo_core::fx::{FxInstance, FxRef, ParamValue, Transition};
    if std::env::var("KOOEDITOR_DEMO_CHAT").is_err() {
        return;
    }
    let clip = demo.ids.selected;
    let mut vignette = FxInstance::new(FxRef::builtin("vignette"));
    vignette
        .params
        .insert("amount".into(), ParamValue::Number(0.35));
    let commands = [
        Command::AddEffect {
            clips: vec![clip],
            effect: vignette,
            index: None,
        },
        Command::SetTransition {
            clip,
            edge: Edge::Start,
            transition: Some(Transition::new(
                FxRef::builtin("cross_dissolve"),
                Time::from_secs_f64(0.5),
            )),
        },
        Command::SetAudioEffects {
            clip: demo.ids.music,
            effects: vec![AudioFx::new(AudioFxKind::Compressor)],
        },
    ];
    for command in commands {
        let _ = state.bus.execute(command, Author::User, "demo");
    }
    let p = state.bus.project();
    let Some((track, c)) = p.timeline.clip(clip) else {
        return;
    };
    let effect = c.effects.last().map(|e| e.id);
    let media = c.media();
    let compressor = p
        .timeline
        .clip(demo.ids.music)
        .and_then(|(_, m)| m.audio.effects.first().map(|e| e.id));
    let (Some(effect), Some(media), Some(compressor)) = (effect, media, compressor) else {
        return;
    };
    let marker = p.timeline.markers[0].id;
    let bin = p.bins[0].id;
    let (track_id, name) = (track.id, c.display_name(p));
    let text = format!(
        "Warm up @clip {clip} to match @media {media}, lower @effect {clip}/{effect} \
         and swap @transition {clip}/in for @fx dip_to_color.\n\
         Ease @effect {}/{compressor} like @audiofx limiter on @track {track_id}, \
         keep @marker {marker} and @bin {bin}, \
         tighten @range 0.600-3.400 and start at @time 11.400",
        demo.ids.music
    );
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    state.chat.push(msg(ChatRole::User, &text, now));
    state.mention(
        vec![
            (format!("@clip {clip}"), name.into()),
            ("@fx vignette".into(), "Vignette".into()),
            (format!("@effect {clip}/{effect}"), "Vignette".into()),
            (
                "@range 0.600-3.400".into(),
                "“I never planned to make films.”".into(),
            ),
            ("@time 11.400".into(), "0:11.4".into()),
            ("@audiofx compressor".into(), "Compressor".into()),
        ],
        cx,
    );
}
