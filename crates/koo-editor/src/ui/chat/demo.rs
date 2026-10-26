//! Development seed behind `KOOEDITOR_DEMO=1`: a small interview project, a
//! conversation with one accepted and one pending change set (both built
//! through the real bus), an open question and a few activity entries.

use crate::state::EditorState;
use koo_core::agent::{ActivityEntry, ChatMessage, ChatRole};
use koo_core::bus::{AgentRole, Author, CommandBus};
use koo_core::command::Command;
use koo_core::model::*;
use koo_core::{BinId, MarkerId, MediaId, Rate, Time, TimeRange};
use serde_json::json;
use std::path::PathBuf;

pub fn enabled() -> bool {
    std::env::var("KOOEDITOR_DEMO").is_ok_and(|v| v == "1")
}

fn t(secs: f64) -> Time {
    Time::from_secs_f64(secs)
}

fn range(a: f64, b: f64) -> TimeRange {
    TimeRange::new(t(a), t(b))
}

pub fn seed(state: &mut EditorState) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (project, captions, music_clip) = project();
    let mut bus = CommandBus::new(project);
    bus.set_permissions(state.settings.permissions.clone());
    let agent = Author::Agent {
        role: AgentRole::Editor,
    };

    let mut chat = vec![msg(
        ChatRole::User,
        "Put my name on screen at the start, and the episode title right after",
        now - 900,
    )];
    let mut first = None;
    for (r, text, reason) in [
        (
            range(2.0, 6.0),
            "Maya Chen — Product Designer",
            "Name caption over the intro",
        ),
        (
            range(6.5, 10.0),
            "Designing for the timeline",
            "Episode title after the name",
        ),
    ] {
        if let Ok(koo_core::bus::Outcome::Proposed { change_set, .. }) = bus.execute(
            Command::AddCaption {
                track: captions,
                range: r,
                text: text.into(),
                speaker: None,
            },
            agent.clone(),
            reason,
        ) {
            first = Some(change_set);
        }
    }
    if let Some(set) = first {
        bus.close_change_set(set);
        let _ = bus.accept(set, None);
        chat.push(msg(
            ChatRole::Agent,
            "Added both as captions on **C1**, styled with your default preset.",
            now - 880,
        ));
        chat.push(ChatMessage {
            change_set: Some(set),
            ..msg(ChatRole::Agent, "", now - 880)
        });
    }

    chat.push(msg(
        ChatRole::User,
        "remove the long pauses where nothing happens, and zoom in whenever I point something out",
        now - 420,
    ));
    // Latest first, so every range is still in original timeline time.
    let pauses = [
        (512.4, 515.1, "Pause 2.7 s after the pricing question"),
        (431.0, 433.2, "Pause 2.2 s, looking at notes"),
        (318.6, 321.9, "Pause 3.3 s before the demo"),
        (247.2, 249.0, "Pause 1.8 s inside the sponsor read"),
        (164.8, 167.4, "Pause 2.6 s, reaching for the mug"),
        (41.3, 43.9, "Pause 2.6 s after the intro"),
    ];
    let mut pending = None;
    let title = "Remove 6 long pauses and mark 2 zoom moments";
    for (ix, (a, b, reason)) in pauses.iter().enumerate() {
        let reason = if ix == 0 { title } else { reason };
        if let Ok(koo_core::bus::Outcome::Proposed { change_set, .. }) = bus.execute(
            Command::RippleDelete {
                range: range(*a, *b),
                tracks: None,
            },
            agent.clone(),
            reason,
        ) {
            pending = Some(change_set);
        }
    }
    for (a, note, reason) in [
        (
            96.0,
            "Zoom 1.8× on the chart",
            "Pointing at the revenue chart",
        ),
        (
            288.5,
            "Zoom 1.8× on the sketch",
            "Pointing at the whiteboard sketch",
        ),
    ] {
        let _ = bus.execute(
            Command::AddMarker {
                range: range(a, a + 2.5),
                kind: MarkerKind::Note,
                note: note.into(),
            },
            agent.clone(),
            reason,
        );
    }
    let _ = bus.execute(
        Command::SetGain {
            clip: music_clip,
            db: -6.0,
        },
        agent.clone(),
        "Music −6 dB under the 3:10 explanation",
    );
    if let Some(set) = pending {
        bus.close_change_set(set);
        chat.push(msg(
            ChatRole::Agent,
            "Found **6 pauses** (15.2 s) and **2 moments** where you point at the screen. \
             Zooms aren't available in this build yet, so I marked those two moments instead. \
             One pause sits inside the protected sponsor read.",
            now - 400,
        ));
        chat.push(ChatMessage {
            change_set: Some(set),
            ..msg(ChatRole::Agent, "", now - 400)
        });
    }
    chat.push(ChatMessage {
        choices: vec![
            "Match the last video".into(),
            "Default style".into(),
            "Skip captions".into(),
        ],
        ..msg(
            ChatRole::Agent,
            "Should the rest of the captions match your last video's style?",
            now - 395,
        )
    });

    state.bus = bus;
    state.chat = chat;
    state.activity = activity(now);
}

fn msg(role: ChatRole, text: &str, at: i64) -> ChatMessage {
    ChatMessage {
        role,
        text: text.into(),
        choices: vec![],
        change_set: None,
        at,
    }
}

/// Returns the project, the caption track and the music clip.
fn project() -> (Project, koo_core::TrackId, koo_core::ClipId) {
    let mut p = Project::new("Designing for the timeline");
    let interviews = BinId::new();
    let music = BinId::new();
    p.bins = vec![
        Bin {
            id: interviews,
            name: "Interviews".into(),
            parent: None,
        },
        Bin {
            id: music,
            name: "Music".into(),
            parent: None,
        },
    ];
    let cam = media(
        "maya-interview.mp4",
        MediaKind::Video,
        600.0,
        Some(interviews),
    );
    let broll = media("desk-broll.mp4", MediaKind::Video, 95.0, Some(interviews));
    let bed = media("lofi-bed.mp3", MediaKind::Audio, 610.0, Some(music));
    let (cam_id, broll_id, bed_id) = (cam.id, broll.id, bed.id);
    p.media = vec![cam, broll, bed];

    let source = |media: MediaId, at: f64| ClipContent::Media {
        media,
        source_in: t(at),
        audio_stream: None,
    };
    let tl = &mut p.timeline;
    let captions = tl.tracks[0].id;
    tl.tracks[1].clips = vec![
        Clip::new(t(0.0), t(300.0), source(cam_id, 0.0)),
        Clip::new(t(300.0), t(12.0), source(broll_id, 20.0)),
        Clip::new(t(312.0), t(288.0), source(cam_id, 312.0)),
    ];
    tl.tracks[2].clips = vec![Clip::new(t(0.0), t(600.0), source(cam_id, 0.0))];
    let mut music_track = Track::new(TrackKind::Audio, "A2");
    let bed_clip = Clip::new(t(0.0), t(600.0), source(bed_id, 0.0));
    let music_clip = bed_clip.id;
    music_track.clips = vec![bed_clip];
    tl.tracks.push(music_track);
    tl.markers = vec![
        Marker {
            id: MarkerId::new(),
            range: range(240.0, 270.0),
            kind: MarkerKind::Protected,
            note: "Sponsor read".into(),
        },
        Marker {
            id: MarkerId::new(),
            range: range(190.0, 190.0),
            kind: MarkerKind::Note,
            note: "Best quote".into(),
        },
    ];
    (p, captions, music_clip)
}

fn media(name: &str, kind: MediaKind, secs: f64, bin: Option<BinId>) -> MediaItem {
    MediaItem {
        id: MediaId::new(),
        path: PathBuf::from("/demo").join(name),
        kind,
        fingerprint: None,
        info: Some(MediaInfo {
            duration: t(secs),
            video: (kind == MediaKind::Video).then_some(VideoStream {
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
        bin,
        tags: vec![],
        proxy: None,
    }
}

fn activity(now: i64) -> Vec<ActivityEntry> {
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
            "read_project",
            json!({}),
            "1 timeline, 4 tracks, 6 clips, 2 markers",
        ),
        entry(
            now - 416,
            "find_silences",
            json!({ "min_duration": 1.5, "threshold_db": -42 }),
            "6 silences ≥ 1.5 s, 15.2 s total",
        ),
        ActivityEntry {
            excerpts: vec![
                "1:36  “…and if you look right here, revenue doubles in March…”".into(),
                "4:48  “…this sketch is where it started, see this corner?”".into(),
            ],
            ..entry(
                now - 414,
                "search_transcript",
                json!({ "query": "look here | see this | right here" }),
                "2 pointing moments",
            )
        },
        ActivityEntry {
            images: vec![collage_path("1-36"), collage_path("4-48")],
            ..entry(
                now - 410,
                "view_range",
                json!({ "start": "1:34.000", "end": "1:40.000", "frames": 30 }),
                "30-frame collage, hand on the chart at 1:36.4",
            )
        },
        ActivityEntry {
            ok: false,
            ..entry(
                now - 406,
                "add_zoom",
                json!({ "clip": "v1", "start": 96.0, "end": 98.5, "scale": 1.8 }),
                "Not implemented yet",
            )
        },
        entry(
            now - 404,
            "ripple_delete",
            json!({ "start": "8:32.400", "end": "8:35.100" }),
            "Proposed, 6 cuts",
        ),
        entry(
            now - 402,
            "set_gain",
            json!({ "clip": "a2", "db": -6.0 }),
            "Proposed",
        ),
    ]
}

/// Collage thumbnails would come from `MediaService::collage`; the demo has
/// no media, so these paths only exercise the layout.
fn collage_path(name: &str) -> PathBuf {
    koo_core::paths::cache().join(format!("demo-collage-{name}.png"))
}
