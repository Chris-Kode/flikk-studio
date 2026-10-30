//! Read-analysis tools: transcript, speakers, silences, shot changes, all
//! mapped from source time to timeline time through the clips that use it.

use super::{Done, Step, ToolCtx};
use crate::args::{Args, ToolError, range_json, secs, short};
use koo_core::analysis::{Analysis, Speaker};
use koo_core::memory::{FactStatus, SourceFact};
use koo_core::model::{ClipContent, Project, TrackKind};
use koo_core::services::Services;
use koo_core::{SpeakerId, Time, TimeRange};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

/// Words returned by one `get_transcript` call before it is cut short.
const MAX_WORDS: usize = 4000;
/// Silences or shot changes listed by one call before it is cut short.
const MAX_LISTED: usize = 500;
/// A pause this long starts a new transcript line.
const LINE_PAUSE: Time = Time(705_600_000);

#[derive(Clone, Debug)]
pub struct TlWord {
    pub range: TimeRange,
    pub text: String,
    pub speaker: Option<SpeakerId>,
    pub confidence: f32,
}

type Analysed<'p> = Vec<(TimeRange, &'p koo_core::model::Clip, Arc<Analysis>)>;

/// Media clips on video and audio tracks overlapping `range`, with their
/// analysis, as (clip timeline range ∩ range, clip, analysis).
fn analysed_clips<'p>(
    project: &'p Project,
    services: &Services,
    range: TimeRange,
) -> Result<Analysed<'p>, ToolError> {
    let Some(service) = services.analysis.as_ref() else {
        return Err(ToolError(
            "Analysis is not available in this build yet.".into(),
        ));
    };
    let mut out = Vec::new();
    for track in &project.timeline.tracks {
        if !matches!(track.kind, TrackKind::Video | TrackKind::Audio) {
            continue;
        }
        for clip in &track.clips {
            let Some(span) = clip.range().intersect(&range) else {
                continue;
            };
            let Some(media) = clip.media().and_then(|m| project.media(m)) else {
                continue;
            };
            let Some(analysis) = media.fingerprint.as_ref().and_then(|f| service.analysis(f))
            else {
                continue;
            };
            out.push((span, clip, analysis));
        }
    }
    Ok(out)
}

/// Transcript words in `range`, in timeline time, deduplicated across the
/// video and audio clips of the same source.
pub fn timeline_words(
    project: &Project,
    services: &Services,
    range: TimeRange,
) -> Result<(Vec<TlWord>, Vec<Speaker>), ToolError> {
    let clips = analysed_clips(project, services, range)?;
    let mut seen = HashSet::new();
    let mut words = Vec::new();
    let mut speakers: Vec<Speaker> = Vec::new();
    for (span, clip, analysis) in &clips {
        let Some(transcript) = &analysis.transcript else {
            continue;
        };
        for s in &transcript.speakers {
            if !speakers.iter().any(|x| x.id == s.id) {
                speakers.push(s.clone());
            }
        }
        let (Some(src_start), Some(src_end)) =
            (clip.to_source(span.start), clip.to_source(span.end))
        else {
            continue;
        };
        for w in transcript.words_in(TimeRange::new(src_start, src_end)) {
            let start = span.start + (w.range.start - src_start);
            let tl = TimeRange::new(start, start + w.range.duration());
            if !span.contains(tl.start) {
                continue;
            }
            if seen.insert((tl.start.as_millis(), w.text.clone())) {
                words.push(TlWord {
                    range: tl,
                    text: w.text.clone(),
                    speaker: w.speaker,
                    confidence: w.confidence,
                });
            }
        }
    }
    words.sort_by_key(|w| w.range.start);
    Ok((words, speakers))
}

fn speaker_label(speakers: &[Speaker], id: Option<SpeakerId>) -> String {
    match id {
        Some(id) => speakers
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.label.clone())
            .unwrap_or_else(|| short(id)),
        None => "?".into(),
    }
}

/// Lines of `[start–end] Speaker: word@t word@t …`.
pub fn transcript_lines(words: &[TlWord], speakers: &[Speaker], with_times: bool) -> Vec<String> {
    transcript_entries(words, speakers, with_times)
        .into_iter()
        .map(|(_, line)| line)
        .collect()
}

/// [`transcript_lines`] with the timeline time each line starts at.
pub fn transcript_entries(
    words: &[TlWord],
    speakers: &[Speaker],
    with_times: bool,
) -> Vec<(Time, String)> {
    let mut lines = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let mut j = i + 1;
        while j < words.len()
            && words[j].speaker == words[i].speaker
            && words[j].range.start - words[j - 1].range.end < LINE_PAUSE
            && j - i < 40
        {
            j += 1;
        }
        let text: Vec<String> = words[i..j]
            .iter()
            .map(|w| {
                let low = if w.confidence < 0.5 { "?" } else { "" };
                if with_times {
                    format!("{}{low}@{}", w.text, secs(w.range.start))
                } else {
                    format!("{}{low}", w.text)
                }
            })
            .collect();
        lines.push((
            words[i].range.start,
            format!(
                "[{}–{}] {}: {}",
                words[i].range.start,
                words[j - 1].range.end,
                speaker_label(speakers, words[i].speaker),
                text.join(" ")
            ),
        ));
        i = j;
    }
    lines
}

/// Confirmed facts about sources used in `range` (PRD F1), fetched by range.
pub fn facts_for(project: &Project, facts: &[SourceFact], range: TimeRange) -> Vec<Value> {
    let mut out = Vec::new();
    for track in &project.timeline.tracks {
        for clip in track.clips.iter().filter(|c| c.range().overlaps(&range)) {
            let Some(media) = clip.media().and_then(|m| project.media(m)) else {
                continue;
            };
            let Some(fp) = &media.fingerprint else {
                continue;
            };
            for f in facts
                .iter()
                .filter(|f| &f.fingerprint == fp && f.status == FactStatus::Confirmed)
            {
                let in_range = f.range.is_none_or(|r| {
                    clip.to_timeline(r.start)
                        .or(clip.to_timeline(r.end))
                        .is_some_and(|t| range.contains(t))
                });
                let v = json!({ "media": short(media.id), "kind": f.kind, "fact": f.text });
                if in_range && !out.contains(&v) {
                    out.push(v);
                }
            }
        }
    }
    out
}

pub fn get_transcript(ctx: &mut ToolCtx, args: &Args) -> Result<Step, ToolError> {
    let project = ctx.project();
    let range = args.range("range")?;
    // Times on every word cost more than the words; only cuts need them.
    let word_times = args.opt_bool("word_times")?.unwrap_or(false);
    let (mut words, speakers) = timeline_words(project, ctx.services, range)?;
    let truncated = words.len() > MAX_WORDS;
    words.truncate(MAX_WORDS);
    let lines = transcript_lines(&words, &speakers, word_times);
    let mut text = if lines.is_empty() {
        format!("No transcribed speech in {range}.")
    } else {
        lines.join("\n")
    };
    if truncated {
        text.push_str(&format!(
            "\n(cut short after {MAX_WORDS} words; ask for a smaller range)"
        ));
    }
    let facts = facts_for(project, &ctx.memory.facts, range);
    if !facts.is_empty() {
        text.push_str("\n\nSource facts:\n");
        text.push_str(&serde_json::to_string(&facts).unwrap_or_default());
    }
    let plain = transcript_lines(&words, &speakers, false).join("\n");
    let mut done = Done::text(format!("transcript {range}: {} words", words.len()), text);
    done.excerpts.push(plain);
    Ok(done.into())
}

fn normalize(word: &str) -> String {
    word.chars()
        .filter(|c| c.is_alphanumeric() || *c == '\'')
        .flat_map(char::to_lowercase)
        .collect()
}

pub fn search_transcript(ctx: &mut ToolCtx, args: &Args) -> Result<Step, ToolError> {
    let project = ctx.project();
    let query = args.str("text")?;
    let range = args.range_or_all("range", project)?;
    let needle: Vec<String> = query
        .split_whitespace()
        .map(normalize)
        .filter(|w| !w.is_empty())
        .collect();
    if needle.is_empty() {
        return Err(ToolError("`text` has no words".into()));
    }
    let (words, speakers) = timeline_words(project, ctx.services, range)?;
    let norm: Vec<String> = words.iter().map(|w| normalize(&w.text)).collect();
    let mut hits = Vec::new();
    let mut excerpts = Vec::new();
    let mut i = 0;
    while i + needle.len() <= words.len() && hits.len() < 50 {
        // Last word may be a prefix ("edit" finds "editing").
        let matched = needle.iter().enumerate().all(|(k, n)| {
            let w = &norm[i + k];
            if k + 1 == needle.len() {
                w.starts_with(n.as_str())
            } else {
                w == n
            }
        });
        if !matched {
            i += 1;
            continue;
        }
        let end = i + needle.len();
        let context: Vec<_> = words[i.saturating_sub(6)..(end + 6).min(words.len())]
            .iter()
            .map(|w| w.text.as_str())
            .collect();
        let context = context.join(" ");
        hits.push(json!({
            "start": secs(words[i].range.start),
            "end": secs(words[end - 1].range.end),
            "speaker": speaker_label(&speakers, words[i].speaker),
            "context": context,
        }));
        excerpts.push(format!("[{}] {context}", words[i].range.start));
        i = end;
    }
    let n = hits.len();
    let mut done = Done::json(
        format!("\"{query}\": {n} matches"),
        &json!({ "query": query, "matches": hits }),
    );
    done.excerpts = excerpts;
    Ok(done.into())
}

pub fn get_speakers(ctx: &mut ToolCtx, args: &Args) -> Result<Step, ToolError> {
    let project = ctx.project();
    let range = args.range_or_all("range", project)?;
    let (words, speakers) = timeline_words(project, ctx.services, range)?;
    let mut stats: BTreeMap<String, (Time, Time, usize)> = BTreeMap::new();
    for w in &words {
        let key = w.speaker.map(short).unwrap_or_else(|| "?".into());
        let e = stats.entry(key).or_insert((Time::ZERO, w.range.start, 0));
        e.0 = e.0 + w.range.duration();
        e.2 += 1;
    }
    let list: Vec<_> = stats
        .iter()
        .map(|(id, (talk, first, n))| {
            let label = speakers
                .iter()
                .find(|s| short(s.id) == *id)
                .map(|s| s.label.clone())
                .unwrap_or_else(|| "unknown".into());
            json!({ "id": id, "label": label, "talk_time": secs(*talk), "first": secs(*first), "words": n })
        })
        .collect();
    let n = list.len();
    let facts = facts_for(project, &ctx.memory.facts, range);
    Ok(Done::json(
        format!("{n} speakers in {range}"),
        &json!({ "range": range_json(range), "speakers": list, "source_facts": facts }),
    )
    .into())
}

pub fn find_silences(ctx: &mut ToolCtx, args: &Args) -> Result<Step, ToolError> {
    let project = ctx.project();
    let range = args.range_or_all("range", project)?;
    let min = Time::from_secs_f64(args.opt_f64("min_s")?.unwrap_or(0.5).max(0.05));
    let Some(service) = ctx.services.analysis.as_ref() else {
        return Err(ToolError(
            "Analysis is not available in this build yet.".into(),
        ));
    };
    let mut gaps: Vec<TimeRange> = Vec::new();
    for track in &project.timeline.tracks {
        if !matches!(track.kind, TrackKind::Video | TrackKind::Audio) {
            continue;
        }
        for clip in &track.clips {
            let Some(span) = clip.range().intersect(&range) else {
                continue;
            };
            let ClipContent::Media { media, .. } = &clip.content else {
                continue;
            };
            let Some(item) = project.media(*media) else {
                continue;
            };
            let silences = service.silences(item, min).map_err(ToolError::from)?;
            for s in silences {
                let start = clip.start + (s.start - clip.to_source(clip.start).unwrap_or(s.start));
                let tl = TimeRange::new(start, start + s.duration());
                if let Some(cut) = tl.intersect(&span).filter(|r| r.duration() >= min) {
                    gaps.push(cut);
                }
            }
        }
    }
    gaps.sort_by_key(|g| g.start);
    gaps.dedup();
    let total = gaps.iter().fold(Time::ZERO, |acc, g| acc + g.duration());
    let list: Vec<_> = gaps
        .iter()
        .take(MAX_LISTED)
        .map(|g| json!([secs(g.start), secs(g.end)]))
        .collect();
    let mut out = json!({ "silences": list, "count": gaps.len(), "total": secs(total) });
    if gaps.len() > MAX_LISTED {
        out["truncated"] = json!(format!(
            "first {MAX_LISTED}; ask for a smaller range or a longer `min_s`"
        ));
    }
    Ok(Done::json(
        format!(
            "{} silences ≥ {:.2} s in {range}",
            gaps.len(),
            min.as_secs_f64()
        ),
        &out,
    )
    .into())
}

pub fn find_shot_changes(ctx: &mut ToolCtx, args: &Args) -> Result<Step, ToolError> {
    let project = ctx.project();
    let range = args.range_or_all("range", project)?;
    let clips = analysed_clips(project, ctx.services, range)?;
    let mut cuts: Vec<Time> = Vec::new();
    for (span, clip, analysis) in clips {
        let Some(track) = project.timeline.clip(clip.id).map(|(t, _)| t) else {
            continue;
        };
        if track.kind != TrackKind::Video {
            continue;
        }
        cuts.extend(
            analysis
                .shot_changes
                .iter()
                .filter_map(|s| clip.to_timeline(*s))
                .filter(|t| span.contains(*t)),
        );
        // Edit points are shot changes too.
        if range.contains(clip.start) && clip.start > Time::ZERO {
            cuts.push(clip.start);
        }
    }
    cuts.sort();
    cuts.dedup_by_key(|t| t.as_millis() / 40);
    let list: Vec<_> = cuts.iter().take(MAX_LISTED).map(|t| secs(*t)).collect();
    let mut out = json!({ "shot_changes": list });
    if cuts.len() > MAX_LISTED {
        out["truncated"] = json!(format!(
            "first {MAX_LISTED} of {}; ask for a smaller range",
            cuts.len()
        ));
    }
    Ok(Done::json(format!("{} shot changes in {range}", cuts.len()), &out).into())
}
