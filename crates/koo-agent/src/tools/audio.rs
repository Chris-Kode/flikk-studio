//! Hearing tools: the model never hears audio, so analysis turns it into
//! the sound script (words, sounds, music with its tempo and changes,
//! silences), beats to cut on, vectors to search by description, and
//! pictures of it. Sounds and music are mapped from source time to the
//! timeline through the clips that play them (`koo_core::heard`).

use super::analysis::{timeline_words, transcript_entries};
use super::view::{image_content, save_png};
use super::{Done, Job, JobDone, Step, ToolCtx};
use crate::args::{Args, ToolError, range_json, secs, short};
use crate::bridge::{Content, ToolResult};
use koo_core::analysis::SoundVectors;
use koo_core::heard::{self, HeardMusic};
use koo_core::model::{Clip, Fingerprint, MediaKind, Project, TrackKind};
use koo_core::services::{
    AnalysisService, AudioLane, AudioMark, AudioMarkKind, AudioPictureRequest, AudioSegment,
    Services,
};
use koo_core::{Time, TimeRange, TrackId};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::Arc;

/// Lines one `get_audio` call returns before it is cut short.
const MAX_LINES: usize = 800;
/// A sound the tagger was less sure of than this is marked "?".
const UNSURE: f32 = 0.35;
/// Quieter than this for at least `SILENCE_MIN` is silence.
const SILENCE_DB: f32 = -50.0;
const SILENCE_MIN: Time = Time(koo_core::time::FLICKS_PER_SECOND / 2);
/// Longest span `view_audio` draws, and how wide.
const PICTURE_MAX: f64 = 30.0;
const PICTURE_WIDTH: u32 = 1400;
/// Beats and downbeats listed by one `get_music` call, per stretch.
const MAX_BEATS: usize = 256;
/// Search matches scoring less are left out.
pub(super) const MIN_SCORE: f32 = 0.1;

/// `x` to `places` decimals, for JSON that reads cleanly.
pub(super) fn round(x: f64, places: i32) -> f64 {
    let k = 10f64.powi(places);
    (x * k).round() / k
}

fn analysis(services: &Services) -> Result<&Arc<dyn AnalysisService>, ToolError> {
    services
        .analysis
        .as_ref()
        .ok_or_else(|| ToolError("Analysis is not available in this build yet.".into()))
}

fn track_name(project: &Project, id: TrackId) -> String {
    project
        .timeline
        .tracks
        .iter()
        .find(|t| t.id == id)
        .map(|t| t.name.clone())
        .unwrap_or_default()
}

/// One line of music for the script: its tags, tempo, first downbeat and bar.
fn music_line(m: &HeardMusic) -> String {
    let mut parts: Vec<String> = m.music.tags.clone();
    if let Some(bpm) = m.music.bpm() {
        parts.push(format!("{bpm:.0} BPM"));
    }
    if let Some(first) = m.downbeats.first() {
        parts.push(format!("downbeat {first}"));
    }
    if let Some(bar) = m.music.bar() {
        parts.push(format!("bar {:.2} s", bar.as_secs_f64()));
    }
    if parts.is_empty() {
        "♪ music".into()
    } else {
        format!("♪ music: {}", parts.join(", "))
    }
}

/// The sound script of `range`: transcript lines, `[sounds]`, `♪` music
/// with its tempo and changes in level, and `(silence)`, in timeline
/// order. Sounds and music carry their track's name when more than one
/// track has them.
pub fn script(
    project: &Project,
    services: &Services,
    range: TimeRange,
) -> Result<Vec<String>, ToolError> {
    let service = analysis(services)?;
    let get = |fp: &Fingerprint| service.analysis(fp);
    // (start, order among lines starting together, line)
    let mut lines: Vec<(Time, u8, String)> = Vec::new();
    let (words, speakers) = timeline_words(project, services, range)?;
    for (t, line) in transcript_entries(&words, &speakers, false) {
        lines.push((t, 2, line));
    }
    let sounds = heard::sounds_in_range(project, range, get);
    let music = heard::music_in_range(project, range, get);
    let tracks: HashSet<TrackId> = sounds
        .iter()
        .map(|s| s.track)
        .chain(music.iter().map(|m| m.track))
        .collect();
    let on = |id: TrackId| {
        if tracks.len() > 1 {
            format!("{}: ", track_name(project, id))
        } else {
            String::new()
        }
    };
    for s in &sounds {
        let unsure = if s.sound.confidence < UNSURE { "?" } else { "" };
        lines.push((
            s.timeline.start,
            3,
            format!(
                "[{}] {}[{}{unsure}]",
                s.timeline,
                on(s.track),
                s.sound.label
            ),
        ));
    }
    for m in &music {
        lines.push((
            m.timeline.start,
            0,
            format!("[{}] {}{}", m.timeline, on(m.track), music_line(m)),
        ));
        let levels = project
            .media(m.media)
            .and_then(|i| i.fingerprint.as_ref())
            .and_then(get);
        if let Some(levels) = levels.as_ref().and_then(|a| a.levels.as_ref()) {
            for (t, db) in heard::level_changes(project, m, levels) {
                let way = if db > 0.0 { "louder" } else { "softer" };
                lines.push((t, 1, format!("[{t}] {}♪ {way}: {db:+.0} dB", on(m.track))));
            }
        }
    }
    for r in heard::silences_in_range(project, range, get, SILENCE_DB, SILENCE_MIN) {
        lines.push((r.start, 4, format!("[{r}] (silence)")));
    }
    lines.sort_by_key(|l| (l.0, l.1));
    Ok(lines.into_iter().map(|l| l.2).collect())
}

/// A line saying how many sources in `range` have no sounds analysed yet.
pub fn unheard_note(project: &Project, services: &Services, range: TimeRange) -> Option<String> {
    let service = services.analysis.as_ref()?;
    let mut media = HashSet::new();
    for track in &project.timeline.tracks {
        if !matches!(track.kind, TrackKind::Audio | TrackKind::Video) {
            continue;
        }
        for clip in track.clips.iter().filter(|c| c.range().overlaps(&range)) {
            media.extend(clip.media());
        }
    }
    let unheard = media
        .iter()
        .filter_map(|m| project.media(*m))
        .filter(|m| {
            m.info
                .as_ref()
                .map_or(m.kind != MediaKind::Image, |i| !i.audio.is_empty())
        })
        .filter(|m| {
            m.fingerprint
                .as_ref()
                .and_then(|fp| service.analysis(fp))
                .is_none()
        })
        .count();
    (unheard > 0).then(|| {
        format!(
            "Sounds and music are not analysed yet for {unheard} source{} here.",
            if unheard == 1 { "" } else { "s" }
        )
    })
}

pub fn get_audio(ctx: &mut ToolCtx, args: &Args) -> Result<Step, ToolError> {
    let project = ctx.project();
    let range = args.range("range")?;
    let mut lines = script(project, ctx.services, range)?;
    let n = lines.len();
    lines.truncate(MAX_LINES);
    let mut text = format!("Audio {range}:\n");
    text.push_str(&if lines.is_empty() {
        "(nothing heard)".to_owned()
    } else {
        lines.join("\n")
    });
    if n > MAX_LINES {
        text.push_str(&format!(
            "\n(cut short after {MAX_LINES} lines; ask for a smaller range)"
        ));
    }
    if let Some(note) = unheard_note(project, ctx.services, range) {
        text.push_str(&format!("\n\n{note}"));
    }
    let mut done = Done::text(format!("audio {range}: {n} lines"), text);
    done.excerpts.push(lines.join("\n"));
    Ok(done.into())
}

pub fn get_music(ctx: &mut ToolCtx, args: &Args) -> Result<Step, ToolError> {
    let project = ctx.project();
    let range = args.range_or_all("range", project)?;
    // Every beat is long and rarely needed: cuts snap to them with `align`.
    let with_beats = args.opt_bool("beats")?.unwrap_or(false);
    let service = analysis(ctx.services)?;
    let get = |fp: &Fingerprint| service.analysis(fp);
    let music = heard::music_in_range(project, range, get);
    let list: Vec<Value> = music
        .iter()
        .map(|m| {
            let changes: Vec<Value> = project
                .media(m.media)
                .and_then(|i| i.fingerprint.as_ref())
                .and_then(get)
                .and_then(|a| {
                    a.levels
                        .as_ref()
                        .map(|l| heard::level_changes(project, m, l))
                })
                .unwrap_or_default()
                .into_iter()
                .map(|(t, db)| json!({ "at": secs(t), "db": round(db as f64, 1) }))
                .collect();
            let times = |ts: &[Time]| {
                ts.iter()
                    .take(MAX_BEATS)
                    .map(|t| secs(*t))
                    .collect::<Vec<_>>()
            };
            let mut v = json!({
                "track": track_name(project, m.track),
                "clip": short(m.clip),
                "range": range_json(m.timeline),
                "tags": m.music.tags,
                "bpm": m.music.bpm().map(|b| round(b as f64, 1)),
                "bar": m.music.bar().map(secs),
                "downbeats": times(&m.downbeats),
                "changes": changes,
            });
            if with_beats {
                v["beats"] = json!(times(&m.beats));
            }
            let (listed, name) = if with_beats {
                (&m.beats, "beats")
            } else {
                (&m.downbeats, "downbeats")
            };
            if listed.len() > MAX_BEATS {
                v["truncated"] = json!(format!(
                    "first {MAX_BEATS} of {} {name}; ask for a smaller range",
                    listed.len()
                ));
            }
            v
        })
        .collect();
    let mut out = json!({ "range": range_json(range), "music": list });
    if let Some(note) = unheard_note(project, ctx.services, range) {
        out["note"] = json!(note);
    }
    Ok(Done::json(format!("{} music in {range}", music.len()), &out).into())
}

/// A source heard in the search range, with its sound vectors.
struct Searched {
    clip: Clip,
    vectors: Arc<SoundVectors>,
}

pub fn search_sounds(ctx: &mut ToolCtx, args: &Args) -> Result<Step, ToolError> {
    let project = ctx.project();
    let range = args.range_or_all("range", project)?;
    let text = args
        .opt_str("text")?
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_owned);
    let like = args.opt_range("like")?;
    if text.is_some() == like.is_some() {
        return Err(ToolError(
            "give `text` (a description) or `like` (a timeline range), one of them".into(),
        ));
    }
    let limit = args.opt_f64("limit")?.unwrap_or(8.0).clamp(1.0, 30.0) as usize;
    let service = analysis(ctx.services)?.clone();
    let mut sources = Vec::new();
    let mut seen = HashSet::new();
    let tl = &project.timeline;
    let tracks = tl
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Audio && !t.mix.mute)
        .chain(tl.tracks.iter().filter(|t| t.kind == TrackKind::Video));
    for track in tracks {
        for clip in track
            .clips
            .iter()
            .filter(|c| c.enabled && c.range().overlaps(&range))
        {
            let Some(item) = clip.media().and_then(|m| project.media(m)) else {
                continue;
            };
            let Some(vectors) = item
                .fingerprint
                .as_ref()
                .and_then(|f| service.sound_vectors(f))
            else {
                continue;
            };
            if seen.insert((item.id, clip.start, clip.source_range())) {
                sources.push(Searched {
                    clip: clip.clone(),
                    vectors,
                });
            }
        }
    }
    if sources.is_empty() {
        return Err(ToolError(format!(
            "Nothing in {range} can be searched by sound yet: sound vectors come with analysis, \
             and this build or this media may not have them."
        )));
    }
    // The mean vector of what plays at the start of `like`.
    let query_like = match like {
        Some(r) => {
            let source = sources
                .iter()
                .find(|s| s.clip.range().contains(r.start))
                .ok_or_else(|| ToolError(format!("No searchable sound at {}.", r.start)))?;
            let (a, b) = (
                source.clip.to_source(r.start),
                source.clip.to_source(r.end.min(source.clip.range().end)),
            );
            let (Some(a), Some(b)) = (a, b) else {
                return Err(ToolError(format!("No source under {r}.")));
            };
            Some(
                source
                    .vectors
                    .mean(TimeRange::new(a, b))
                    .ok_or_else(|| ToolError(format!("{r} is silent or not analysed.")))?,
            )
        }
        None => None,
    };
    let snapshot = Arc::new(project.clone());
    let what = text
        .clone()
        .unwrap_or_else(|| format!("like {}", like.expect("checked")));
    let job: Job = Box::new(move || {
        let query = match (&text, query_like) {
            (Some(t), _) => match service.describe_sound(t) {
                Ok(v) => v,
                Err(e) => return JobDone::Done(Done::error(e.to_string())),
            },
            (None, Some(v)) => v,
            (None, None) => unreachable!("checked above"),
        };
        // Weak matches are noise: below 0.1, or below half the best.
        let mut found = rank(&sources, &query, range, like, limit);
        let best = found.first().map_or(0.0, |f| f.1);
        found.retain(|f| f.1 >= MIN_SCORE && f.1 >= best / 2.0);
        let get = |fp: &Fingerprint| service.analysis(fp);
        let matches: Vec<Value> = found
            .iter()
            .map(|(r, score)| {
                let heard: Vec<String> = heard::sounds_in_range(&snapshot, *r, get)
                    .into_iter()
                    .map(|s| s.sound.label)
                    .chain(
                        heard::music_in_range(&snapshot, *r, get)
                            .into_iter()
                            .map(|_| "music".to_owned()),
                    )
                    .collect();
                let mut v = json!({
                    "start": secs(r.start),
                    "end": secs(r.end),
                    "score": round(*score as f64, 2),
                });
                if !heard.is_empty() {
                    v["heard"] = json!(heard);
                }
                v
            })
            .collect();
        let out = json!({
            "query": what,
            "matches": matches,
            "searched": range_json(range),
            "note": "Scores are cosine similarity: compare them with each other; a clear match \
                     is usually above 0.25. Edges are good to about 2 s; get_audio or view_audio \
                     shows exactly where.",
        });
        JobDone::Done(Done::json(
            format!("sounds {what:?}: {} matches", found.len()),
            &out,
        ))
    });
    Ok(Step::Job(job))
}

/// The best stretches of `range` for `query`, at most `limit`, best first,
/// leaving out what overlaps `exclude`. Each 2 s step scores as the lower
/// of the two 4 s windows over it, which places a sound to about 2 s; a
/// match grows over neighbouring steps that score nearly as well.
fn rank(
    sources: &[Searched],
    query: &[f32],
    range: TimeRange,
    exclude: Option<TimeRange>,
    limit: usize,
) -> Vec<(TimeRange, f32)> {
    // (timeline range, score) for every step of every source, in order.
    let mut steps: Vec<Vec<(TimeRange, f32)>> = Vec::new();
    for s in sources {
        let v = &s.vectors;
        let scores = v.scores(query);
        let mut row = Vec::new();
        for k in 0..scores.len() {
            let covering = [k.checked_sub(1), Some(k)];
            let score = covering
                .iter()
                .flatten()
                .map(|&i| scores[i])
                .fold(f32::MAX, f32::min);
            let source = TimeRange::new(Time(v.step.0 * k as i64), Time(v.step.0 * (k as i64 + 1)));
            let Some(on) = s
                .clip
                .source_to_timeline_range(source)
                .intersect(&s.clip.range())
                .and_then(|r| r.intersect(&range))
            else {
                continue;
            };
            if exclude.is_some_and(|x| x.overlaps(&on)) {
                continue;
            }
            row.push((on, score));
        }
        steps.push(row);
    }
    let mut used: Vec<Vec<bool>> = steps.iter().map(|r| vec![false; r.len()]).collect();
    let mut out: Vec<(TimeRange, f32)> = Vec::new();
    while out.len() < limit {
        let best = steps
            .iter()
            .enumerate()
            .flat_map(|(i, row)| row.iter().enumerate().map(move |(k, s)| (i, k, s.1)))
            .filter(|&(i, k, _)| !used[i][k])
            .max_by(|a, b| a.2.total_cmp(&b.2));
        let Some((i, k, score)) = best else {
            break;
        };
        let row = &steps[i];
        let (mut a, mut b) = (k, k);
        while a > 0 && !used[i][a - 1] && row[a - 1].1 >= score * 0.85 {
            a -= 1;
        }
        while b + 1 < row.len() && !used[i][b + 1] && row[b + 1].1 >= score * 0.85 {
            b += 1;
        }
        for u in &mut used[i][a..=b] {
            *u = true;
        }
        let r = TimeRange::new(row[a].0.start, row[b].0.end);
        // The same moment through a linked clip is one match.
        if out.iter().any(|(o, _)| o.overlaps(&r)) {
            continue;
        }
        out.push((r, score));
    }
    out
}

pub fn view_audio(ctx: &mut ToolCtx, args: &Args) -> Result<Step, ToolError> {
    let project = ctx.project();
    let range = args.range("range")?;
    if range.duration().as_secs_f64() > PICTURE_MAX + 0.001 {
        return Err(ToolError(format!(
            "view_audio takes at most {PICTURE_MAX} s; use get_audio for longer spans"
        )));
    }
    let Some(media) = ctx.services.media.clone() else {
        return Err(ToolError(
            "Audio pictures are not available in this build yet.".into(),
        ));
    };
    let lanes = lanes(project, range);
    if lanes.is_empty() {
        return Err(ToolError(format!("No audio in {range}.")));
    }
    let mut marks = Vec::new();
    if let Ok((words, _)) = timeline_words(project, ctx.services, range) {
        marks.extend(words.into_iter().map(|w| AudioMark {
            at: w.range,
            kind: AudioMarkKind::Word,
            label: w.text.trim().to_owned(),
        }));
    }
    if let Some(service) = ctx.services.analysis.as_ref() {
        let get = |fp: &Fingerprint| service.analysis(fp);
        marks.extend(
            heard::sounds_in_range(project, range, get)
                .into_iter()
                .map(|s| AudioMark {
                    at: s.timeline,
                    kind: AudioMarkKind::Sound,
                    label: s.sound.label,
                }),
        );
        for m in heard::music_in_range(project, range, get) {
            let at = |t: Time| TimeRange::new(t, t);
            marks.extend(m.beats.iter().map(|&t| AudioMark {
                at: at(t),
                kind: AudioMarkKind::Beat,
                label: String::new(),
            }));
            marks.extend(m.downbeats.iter().map(|&t| AudioMark {
                at: at(t),
                kind: AudioMarkKind::Downbeat,
                label: String::new(),
            }));
        }
    }
    marks.sort_by_key(|m| m.at.start);
    let lines = script(project, ctx.services, range).unwrap_or_default();
    let header = format!(
        "Audio {range}:\n{}",
        if lines.is_empty() {
            "(nothing heard)".to_owned()
        } else {
            lines.join("\n")
        }
    );
    let request = AudioPictureRequest {
        range,
        lanes,
        marks,
        width: PICTURE_WIDTH,
    };
    let job: Job = Box::new(move || {
        let frame = match media.audio_picture(&request) {
            Ok(f) => f,
            Err(e) => {
                return JobDone::Done(Done::error(format!("Could not draw the audio: {e}")));
            }
        };
        let (path, image) = match save_png(&frame).and_then(|p| Ok((p.clone(), image_content(&p)?)))
        {
            Ok(x) => x,
            Err(ToolError(e)) => return JobDone::Done(Done::error(e)),
        };
        let names: Vec<&str> = request.lanes.iter().map(|l| l.name.as_str()).collect();
        let text = format!(
            "{header}\n\nPicture: time across under a ruler; a spectrogram lane per track ({}), \
             low notes at the bottom, louder is brighter; below them words (white), sounds \
             (amber) and beats (cyan ticks, tall for downbeats, which also run as lines through \
             the lanes).",
            names.join(", ")
        );
        JobDone::Done(Done {
            result: ToolResult {
                content: vec![Content::Text { text }, image],
                is_error: false,
            },
            summary: format!("audio picture {range}"),
            images: vec![path],
            excerpts: vec![lines.join("\n")],
            ..Default::default()
        })
    });
    Ok(Step::Job(job))
}

/// A lane per heard audio track with clips in `range` (the video tracks'
/// clips when no audio track has any), each clip a segment of its source.
fn lanes(project: &Project, range: TimeRange) -> Vec<AudioLane> {
    let lanes_of = |kind: TrackKind| -> Vec<AudioLane> {
        project
            .timeline
            .tracks
            .iter()
            .filter(|t| t.kind == kind && !(kind == TrackKind::Audio && t.mix.mute))
            .filter_map(|t| {
                let segments: Vec<AudioSegment> = t
                    .clips
                    .iter()
                    .filter(|c| c.enabled)
                    .filter_map(|c| {
                        let on = c.range().intersect(&range)?;
                        let item = project.media(c.media()?)?;
                        let (a, b) = (c.to_source(on.start)?, c.to_source(on.end)?);
                        Some(AudioSegment {
                            path: item.path.clone(),
                            stream: 0,
                            source: TimeRange::new(a.min(b), a.max(b)),
                            timeline: on,
                        })
                    })
                    .collect();
                (!segments.is_empty()).then(|| AudioLane {
                    name: t.name.clone(),
                    segments,
                })
            })
            .collect()
    };
    let audio = lanes_of(TrackKind::Audio);
    if audio.is_empty() {
        lanes_of(TrackKind::Video)
    } else {
        audio
    }
}
