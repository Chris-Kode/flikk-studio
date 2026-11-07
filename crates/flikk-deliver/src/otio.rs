//! OpenTimelineIO JSON export (PRD D2). The schema is written by hand:
//! Timeline.1, Stack.1, Track.1, Clip.2, Gap.1, ExternalReference.1,
//! Marker.2, TimeRange.1, RationalTime.1.

use anyhow::Result;
use flikk_core::model::{ClipContent, MarkerKind, Project, TrackKind};
use flikk_core::{Rate, Time};
use serde_json::{Value, json};
use std::path::Path;

fn rational_time(t: Time, rate: Rate) -> Value {
    json!({
        "OTIO_SCHEMA": "RationalTime.1",
        "rate": rate.fps(),
        "value": (t.as_secs_f64() * rate.fps() * 1e6).round() / 1e6,
    })
}

fn time_range(start: Time, duration: Time, rate: Rate) -> Value {
    json!({
        "OTIO_SCHEMA": "TimeRange.1",
        "start_time": rational_time(start, rate),
        "duration": rational_time(duration, rate),
    })
}

/// `file://` URL with percent-encoding; drive paths become `file:///C:/…`.
pub(crate) fn file_url(path: &Path) -> String {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let s = abs.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file://");
    if !s.starts_with('/') {
        out.push('/');
    }
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' | b':' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The project's timeline as an OTIO `Timeline`.
pub fn to_otio(project: &Project) -> Value {
    let tl = &project.timeline;
    let rate = tl.settings.rate;
    // OTIO stacks list the bottom track first; video above, audio after.
    let video = tl
        .tracks
        .iter()
        .rev()
        .filter(|t| matches!(t.kind, TrackKind::Video | TrackKind::Overlay));
    let audio = tl.tracks.iter().filter(|t| t.kind == TrackKind::Audio);
    let tracks: Vec<Value> = video
        .chain(audio)
        .map(|track| {
            let mut children = Vec::new();
            let mut cursor = Time::ZERO;
            for clip in &track.clips {
                let (name, source_in, reference) = match &clip.content {
                    ClipContent::Media {
                        media, source_in, ..
                    } => {
                        let Some(item) = project.media(*media) else {
                            continue;
                        };
                        let available = item
                            .info
                            .as_ref()
                            .map(|i| time_range(Time::ZERO, i.duration, rate))
                            .unwrap_or(Value::Null);
                        (
                            file_name(&item.path),
                            *source_in,
                            json!({
                                "OTIO_SCHEMA": "ExternalReference.1",
                                "name": file_name(&item.path),
                                "target_url": file_url(&item.path),
                                "available_range": available,
                                "available_image_bounds": null,
                                "metadata": {},
                            }),
                        )
                    }
                    ClipContent::Overlay { asset, .. } => (
                        file_name(asset),
                        Time::ZERO,
                        json!({
                            "OTIO_SCHEMA": "ExternalReference.1",
                            "name": file_name(asset),
                            "target_url": file_url(asset),
                            "available_range": null,
                            "available_image_bounds": null,
                            "metadata": {},
                        }),
                    ),
                    _ => continue,
                };
                if clip.start > cursor {
                    children.push(json!({
                        "OTIO_SCHEMA": "Gap.1",
                        "name": "",
                        "source_range": time_range(Time::ZERO, clip.start - cursor, rate),
                        "effects": [],
                        "markers": [],
                        "enabled": true,
                        "metadata": {},
                    }));
                }
                children.push(json!({
                    "OTIO_SCHEMA": "Clip.2",
                    "name": name,
                    "source_range": time_range(source_in, clip.duration, rate),
                    "media_references": { "DEFAULT_MEDIA": reference },
                    "active_media_reference_key": "DEFAULT_MEDIA",
                    "effects": [],
                    "markers": [],
                    "enabled": true,
                    "metadata": { "flikk": {
                        "clip": clip.id.0.to_string(),
                        "transform": clip.transform,
                        "keyframes": clip.keyframes,
                        "audio": clip.audio,
                    }},
                }));
                cursor = clip.end();
            }
            json!({
                "OTIO_SCHEMA": "Track.1",
                "name": track.name,
                "kind": if track.kind == TrackKind::Audio { "Audio" } else { "Video" },
                "children": children,
                "source_range": null,
                "effects": [],
                "markers": [],
                "enabled": !track.mix.mute,
                "metadata": {},
            })
        })
        .collect();
    let markers: Vec<Value> = tl
        .markers
        .iter()
        .map(|m| {
            json!({
                "OTIO_SCHEMA": "Marker.2",
                "name": m.note,
                "marked_range": time_range(m.range.start, m.range.duration(), rate),
                "color": if m.kind == MarkerKind::Protected { "RED" } else { "GREEN" },
                "comment": "",
                "metadata": {},
            })
        })
        .collect();
    json!({
        "OTIO_SCHEMA": "Timeline.1",
        "name": project.name,
        "global_start_time": rational_time(Time::ZERO, rate),
        "tracks": {
            "OTIO_SCHEMA": "Stack.1",
            "name": "tracks",
            "children": tracks,
            "source_range": null,
            "effects": [],
            "markers": markers,
            "enabled": true,
            "metadata": {},
        },
        "metadata": {},
    })
}

/// Write the timeline as `.otio` JSON.
pub fn export_otio(project: &Project, path: &Path) -> Result<()> {
    let text = serde_json::to_string_pretty(&to_otio(project))?;
    crate::ffmpeg::write_atomic(path, text)
}
