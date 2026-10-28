//! What the agent reads about the `@` references in a message: a
//! `<references>` block after the user's text that resolves every token to
//! its ids, timing, source, parameters and transcript, so the agent acts on
//! exactly what the user pointed at. The chat neither shows nor stores it.

use koo_core::audiofx::{AudioFx, AudioFxKind};
use koo_core::command::Edge;
use koo_core::fx::{self, FxInstance, FxLanguage, ParamKind, ParamSpec, ParamValue, Transition};
use koo_core::model::{Clip, ClipContent, MediaItem, MediaKind, Project, Track, TrackKind};
use koo_core::query::{self, Reference};
use koo_core::services::AnalysisService;
use koo_core::{Time, TimeRange};
use std::collections::HashSet;
use std::fmt::Write as _;

/// Words around an `@time` reference, each side.
const TIME_CONTEXT: f64 = 2.0;
/// Longest transcript excerpt, in words.
const EXCERPT_WORDS: usize = 120;

/// The block for the references in `text`, or `None` without any.
pub fn references_block(
    project: &Project,
    analysis: Option<&dyn AnalysisService>,
    text: &str,
) -> Option<String> {
    let mut seen = HashSet::new();
    let mut body = String::new();
    for r in query::resolve_project_references(project, text) {
        let token = &text[r.span.clone()];
        if !token.starts_with('@') || !seen.insert(token.to_owned()) {
            continue;
        }
        let mut out = Out::default();
        describe(&mut out, project, analysis, token, &r.reference);
        body.push_str(&out.0);
    }
    (!body.is_empty()).then(|| {
        format!(
            "<references>\nResolved `@` references in the message above (times in timeline seconds unless marked source).\n{body}</references>"
        )
    })
}

/// Transcript heard under `range`, quoted, or `None` without words there.
pub fn excerpt(
    project: &Project,
    analysis: Option<&dyn AnalysisService>,
    range: TimeRange,
) -> Option<String> {
    let analysis = analysis?;
    let words = query::words_in_range(project, range, |fp| analysis.analysis(fp));
    let mut text: Vec<&str> = words
        .iter()
        .map(|w| w.word.text.trim())
        .filter(|w| !w.is_empty())
        .collect();
    if text.is_empty() {
        return None;
    }
    let cut = text.len() > EXCERPT_WORDS;
    text.truncate(EXCERPT_WORDS);
    Some(format!(
        "\"{}{}\"",
        text.join(" "),
        if cut { " …" } else { "" }
    ))
}

#[derive(Default)]
struct Out(String);

impl Out {
    fn line(&mut self, indent: usize, text: impl AsRef<str>) {
        let _ = writeln!(self.0, "{}{}", "  ".repeat(indent), text.as_ref());
    }
}

fn describe(
    out: &mut Out,
    project: &Project,
    analysis: Option<&dyn AnalysisService>,
    token: &str,
    reference: &Reference,
) {
    let tl = &project.timeline;
    match reference {
        Reference::Clip { clip } => match tl.clip(*clip) {
            Some((track, c)) => {
                out.line(0, format!("{token}: clip \"{}\"", c.display_name(project)));
                clip_lines(out, 1, project, track, c);
            }
            None => missing(out, token),
        },
        Reference::Media { media } => match project.media(*media) {
            Some(m) => {
                out.line(0, format!("{token}: media \"{}\"", stem(&m.path)));
                media_lines(out, 1, project, m);
            }
            None => missing(out, token),
        },
        Reference::Marker { marker } => match tl.marker(*marker) {
            Some(m) => {
                out.line(0, format!("{token}: marker \"{}\"", m.note.trim()));
                out.line(1, format!("range: {}", range(m.range)));
                out.line(1, format!("kind: {}", enum_name(&m.kind)));
                let heard = if m.range.is_empty() {
                    around(m.range.start)
                } else {
                    m.range
                };
                if let Some(words) = excerpt(project, analysis, heard) {
                    out.line(1, format!("transcript {}: {words}", range(heard)));
                }
            }
            None => missing(out, token),
        },
        Reference::Track { track } => match tl.track(*track) {
            Some(t) => {
                out.line(0, format!("{token}: track \"{}\"", t.name));
                track_lines(out, 1, project, t);
            }
            None => missing(out, token),
        },
        Reference::Bin { bin } => match project.bins.iter().find(|b| b.id == *bin) {
            Some(b) => {
                out.line(0, format!("{token}: bin \"{}\"", b.name));
                if let Some(parent) = b.parent {
                    out.line(1, format!("parent: @bin {parent}"));
                }
                let items: Vec<String> = project
                    .media
                    .iter()
                    .filter(|m| m.bin == Some(*bin))
                    .map(|m| format!("@media {} \"{}\"", m.id, stem(&m.path)))
                    .collect();
                out.line(1, format!("media: {}", list_or_none(&items)));
            }
            None => missing(out, token),
        },
        Reference::Effect { clip, effect } => {
            let Some((track, c)) = tl.clip(*clip) else {
                return missing(out, token);
            };
            let on = format!(
                "@clip {} \"{}\" on @track {}",
                c.id,
                c.display_name(project),
                track.id
            );
            if let Some((ix, e)) = c.effects.iter().enumerate().find(|(_, e)| e.id == *effect) {
                out.line(0, format!("{token}: video effect"));
                out.line(1, format!("clip: {on}"));
                out.line(
                    1,
                    format!("stack position: {} of {}", ix + 1, c.effects.len()),
                );
                instance_lines(out, 1, project, e, c.start);
            } else if let Some(e) = c.audio.effects.iter().find(|e| e.id == *effect) {
                out.line(0, format!("{token}: audio effect"));
                out.line(1, format!("clip: {on}"));
                audio_fx_lines(out, 1, e);
            } else {
                missing(out, token);
            }
        }
        Reference::Transition { clip, edge } => {
            let Some((track, c)) = tl.clip(*clip) else {
                return missing(out, token);
            };
            let side = match edge {
                Edge::Start => "in",
                Edge::End => "out",
            };
            out.line(0, format!("{token}: transition {side}"));
            out.line(
                1,
                format!(
                    "clip: @clip {} \"{}\" on @track {}",
                    c.id,
                    c.display_name(project),
                    track.id
                ),
            );
            let t = match edge {
                Edge::Start => c.transition_in.as_ref(),
                Edge::End => c.transition_out.as_ref(),
            };
            match t {
                Some(t) => transition_lines(out, 1, project, track, c, t, *edge),
                None => out.line(1, "transition: none set"),
            }
        }
        Reference::Fx { fx } => match fx::asset(project, fx) {
            Some(a) => {
                out.line(0, format!("{token}: FX asset \"{}\"", a.name));
                out.line(
                    1,
                    format!(
                        "kind: {}, language: {}, {}",
                        enum_name(&a.kind),
                        match a.language {
                            FxLanguage::Glsl => "glsl",
                            FxLanguage::Html => "html",
                        },
                        match fx {
                            fx::FxRef::Builtin(_) => "built-in",
                            fx::FxRef::Project(_) => "project asset",
                        }
                    ),
                );
                if !a.category.is_empty() {
                    out.line(1, format!("category: {}", a.category));
                }
                if !a.description.is_empty() {
                    out.line(1, format!("description: {}", a.description));
                }
                spec_lines(out, 1, &a.params);
                let uses: Vec<String> = fx::uses(project, fx)
                    .into_iter()
                    .map(|c| format!("@clip {c}"))
                    .collect();
                out.line(1, format!("used by: {}", list_or_none(&uses)));
            }
            None => missing(out, token),
        },
        Reference::AudioFx { kind } => {
            out.line(
                0,
                format!("{token}: audio effect type \"{}\"", kind.label()),
            );
            spec_lines(out, 1, &kind.params());
            let uses = audio_fx_uses(project, *kind);
            out.line(1, format!("used by: {}", list_or_none(&uses)));
        }
        Reference::Range { range: r } => {
            out.line(0, format!("{token}: timeline range {}", range(*r)));
            let clips: Vec<String> = tl
                .tracks
                .iter()
                .flat_map(|t| t.clips.iter().map(move |c| (t, c)))
                .filter(|(_, c)| c.range().overlaps(r))
                .map(|(t, c)| {
                    format!(
                        "@clip {} \"{}\" on {} ({})",
                        c.id,
                        c.display_name(project),
                        t.name,
                        range(c.range())
                    )
                })
                .collect();
            out.line(1, format!("clips: {}", list_or_none(&clips)));
            if let Some(words) = excerpt(project, analysis, *r) {
                out.line(1, format!("transcript: {words}"));
            }
        }
        Reference::At { time } => {
            out.line(0, format!("{token}: timeline time {}", secs(*time)));
            let shown: Vec<String> = tl
                .tracks
                .iter()
                .filter_map(|t| t.clip_at(*time).map(|c| (t, c)))
                .map(|(t, c)| {
                    let source = c
                        .to_source(*time)
                        .map(|s| format!(", source {}", secs(s)))
                        .unwrap_or_default();
                    format!(
                        "@clip {} \"{}\" on {}{source}",
                        c.id,
                        c.display_name(project),
                        t.name
                    )
                })
                .collect();
            out.line(1, format!("clips there: {}", list_or_none(&shown)));
            let heard = around(*time);
            if let Some(words) = excerpt(project, analysis, heard) {
                out.line(1, format!("transcript {}: {words}", range(heard)));
            }
        }
        Reference::Unresolved { .. } => missing(out, token),
    }
}

/// [`TIME_CONTEXT`] either side of `t`.
fn around(t: Time) -> TimeRange {
    let pad = Time::from_secs_f64(TIME_CONTEXT);
    TimeRange::new((t - pad).max(Time::ZERO), t + pad)
}

fn missing(out: &mut Out, token: &str) {
    out.line(
        0,
        format!("{token}: not found in the project (deleted or mistyped)"),
    );
}

fn clip_lines(out: &mut Out, i: usize, project: &Project, track: &Track, c: &Clip) {
    out.line(
        i,
        format!(
            "track: @track {} \"{}\" ({})",
            track.id,
            track.name,
            enum_name(&track.kind)
        ),
    );
    out.line(i, format!("timeline: {}", range(c.range())));
    match &c.content {
        ClipContent::Media {
            media,
            source_in,
            audio_stream,
        } => {
            let file = project
                .media(*media)
                .map(|m| m.path.display().to_string())
                .unwrap_or_else(|| "(missing)".into());
            out.line(i, format!("content: @media {media} {file}"));
            if let Some(s) = c.source_range() {
                out.line(i, format!("source: {} (source time)", range(s)));
            } else {
                out.line(i, format!("source in: {}", secs(*source_in)));
            }
            if let Some(stream) = audio_stream {
                out.line(i, format!("audio stream: {stream}"));
            }
        }
        ClipContent::Title { template, text } => {
            out.line(
                i,
                format!("content: title, template {template}, text {text:?}"),
            );
        }
        ClipContent::Caption { text, speaker, .. } => {
            let speaker = speaker
                .map(|s| format!(", speaker {s}"))
                .unwrap_or_default();
            out.line(i, format!("content: caption {text:?}{speaker}"));
        }
        ClipContent::Overlay { asset, .. } => {
            out.line(i, format!("content: overlay {}", asset.display()));
        }
        ClipContent::Generator { fx: g } => {
            out.line(
                i,
                format!("content: generator {}", fx_label(project, &g.fx)),
            );
            instance_lines(out, i + 1, project, g, c.start);
        }
    }
    out.line(
        i,
        format!("speed: {}, enabled: {}", num(c.speed), c.enabled),
    );
    if let Some(name) = &c.name {
        out.line(i, format!("name: {name:?}"));
    }
    if let Some(color) = c.color {
        out.line(i, format!("color: {}", color.label()));
    }
    if c.effects.is_empty() {
        out.line(i, "effects: none");
    } else {
        out.line(i, "effects (applied in order):");
        for e in &c.effects {
            out.line(
                i + 1,
                format!("- @effect {}/{} {}", c.id, e.id, fx_label(project, &e.fx)),
            );
            instance_lines(out, i + 2, project, e, c.start);
        }
    }
    for (edge, t) in [
        (Edge::Start, &c.transition_in),
        (Edge::End, &c.transition_out),
    ] {
        let side = if edge == Edge::Start { "in" } else { "out" };
        match t {
            Some(t) => {
                out.line(i, format!("transition {side}: @transition {}/{side}", c.id));
                transition_lines(out, i + 1, project, track, c, t, edge);
            }
            None => out.line(i, format!("transition {side}: none")),
        }
    }
    if track.kind == TrackKind::Audio || !c.audio.effects.is_empty() || c.audio.gain_db != 0.0 {
        out.line(
            i,
            format!(
                "audio: gain {} dB, fade in {}, fade out {}",
                num(c.audio.gain_db as f64),
                secs(c.audio.fade_in),
                secs(c.audio.fade_out)
            ),
        );
        for e in &c.audio.effects {
            out.line(
                i + 1,
                format!("- @effect {}/{} {}", c.id, e.id, e.kind.name()),
            );
            audio_fx_lines(out, i + 2, e);
        }
    }
    if !c.keyframes.is_empty() {
        let mut props: Vec<&str> = c.keyframes.iter().map(|k| k.property.name()).collect();
        props.dedup();
        out.line(i, format!("keyframed: {}", props.join(", ")));
    }
}

fn media_lines(out: &mut Out, i: usize, project: &Project, m: &MediaItem) {
    let kind = match m.kind {
        MediaKind::Video => "video",
        MediaKind::Audio => "audio",
        MediaKind::Image => "image",
    };
    out.line(i, format!("kind: {kind}"));
    out.line(i, format!("file: {}", m.path.display()));
    if let Some(info) = &m.info {
        out.line(i, format!("duration: {}", secs(info.duration)));
        if let Some(v) = &info.video {
            out.line(
                i,
                format!(
                    "video stream {}: {}x{}, {} fps, {}",
                    v.index,
                    v.width,
                    v.height,
                    num(v.rate.fps()),
                    v.codec
                ),
            );
        }
        for a in &info.audio {
            let lang = a
                .language
                .as_deref()
                .map(|l| format!(", {l}"))
                .unwrap_or_default();
            out.line(
                i,
                format!(
                    "audio stream {}: {} ch, {} Hz, {}{lang}",
                    a.index, a.channels, a.sample_rate, a.codec
                ),
            );
        }
    }
    if let Some(bin) = m.bin {
        out.line(i, format!("bin: @bin {bin}"));
    }
    if !m.tags.is_empty() {
        out.line(i, format!("tags: {}", m.tags.join(", ")));
    }
    let uses: Vec<String> = project
        .timeline
        .tracks
        .iter()
        .flat_map(|t| &t.clips)
        .filter(|c| c.media() == Some(m.id))
        .map(|c| format!("@clip {} ({})", c.id, range(c.range())))
        .collect();
    out.line(i, format!("on the timeline: {}", list_or_none(&uses)));
}

fn track_lines(out: &mut Out, i: usize, project: &Project, t: &Track) {
    out.line(i, format!("kind: {}", enum_name(&t.kind)));
    out.line(
        i,
        format!(
            "mix: volume {} dB, pan {}, mute {}, solo {}, locked {}",
            num(t.mix.volume_db as f64),
            num(t.mix.pan as f64),
            t.mix.mute,
            t.mix.solo,
            t.locked
        ),
    );
    for e in &t.mix.effects {
        out.line(i, format!("track effect: {}", e.kind.name()));
        audio_fx_lines(out, i + 1, e);
    }
    let clips: Vec<String> = t
        .clips
        .iter()
        .map(|c| {
            format!(
                "@clip {} \"{}\" ({})",
                c.id,
                c.display_name(project),
                range(c.range())
            )
        })
        .collect();
    out.line(i, format!("clips: {}", list_or_none(&clips)));
}

fn transition_lines(
    out: &mut Out,
    i: usize,
    project: &Project,
    track: &Track,
    c: &Clip,
    t: &Transition,
    edge: Edge,
) {
    out.line(i, format!("asset: {}", fx_label(project, &t.fx.fx)));
    out.line(
        i,
        format!(
            "duration: {}, align: {}",
            secs(t.duration),
            enum_name(&t.align)
        ),
    );
    // Where it actually plays: an incoming clip's transition wins at a cut.
    let at = match edge {
        Edge::Start => c.start,
        Edge::End => c.end(),
    };
    let placed = fx::transitions(track)
        .into_iter()
        .find(|p| p.at == at && std::ptr::eq(p.transition, t));
    if let Some(p) = &placed {
        let from = p
            .from
            .map_or("nothing".into(), |c| format!("@clip {}", c.id));
        let to = p.to.map_or("nothing".into(), |c| format!("@clip {}", c.id));
        out.line(i, format!("plays: {} from {from} to {to}", range(p.range)));
    } else {
        out.line(
            i,
            "plays: no (overridden at this cut, or the clip is disabled)",
        );
    }
    // Its keyframes count from the transition's start.
    let origin = placed.map_or(at, |p| p.range.start);
    instance_lines(out, i, project, &t.fx, origin);
}

/// Parameter values and enabled state. Keyframe times are relative to
/// `origin` in the model and shown in timeline seconds.
fn instance_lines(out: &mut Out, i: usize, project: &Project, e: &FxInstance, origin: Time) {
    let specs = fx::asset(project, &e.fx)
        .map(|a| a.params.clone())
        .unwrap_or_default();
    let mut values: Vec<String> = specs
        .iter()
        .map(|s| {
            let v = e.params.get(&s.name).unwrap_or(&s.default);
            format!("{}={}", s.name, value(v))
        })
        .collect();
    values.extend(
        e.params
            .iter()
            .filter(|(k, _)| !specs.iter().any(|s| &s.name == *k))
            .map(|(k, v)| format!("{k}={}", value(v))),
    );
    out.line(
        i,
        format!(
            "id: {}, enabled: {}, params: {}",
            e.id,
            e.enabled,
            if values.is_empty() {
                "none".into()
            } else {
                values.join(", ")
            }
        ),
    );
    if !e.keyframes.is_empty() {
        let keys: Vec<String> = e
            .keyframes
            .iter()
            .map(|k| format!("{}@{}={}", k.param, secs(origin + k.at), value(&k.value)))
            .collect();
        out.line(i, format!("keyframes: {}", keys.join(", ")));
    }
}

fn audio_fx_lines(out: &mut Out, i: usize, e: &AudioFx) {
    let values: Vec<String> = e
        .kind
        .params()
        .iter()
        .map(|s| {
            let v = e
                .params
                .get(&s.name)
                .copied()
                .or_else(|| s.default.as_f64())
                .unwrap_or_default();
            format!("{}={}", s.name, num(v))
        })
        .collect();
    out.line(
        i,
        format!(
            "kind: {}, id: {}, enabled: {}, params: {}",
            e.kind.name(),
            e.id,
            e.enabled,
            values.join(", ")
        ),
    );
}

fn spec_lines(out: &mut Out, i: usize, specs: &[ParamSpec]) {
    if specs.is_empty() {
        return out.line(i, "params: none");
    }
    out.line(i, "params:");
    for s in specs {
        let kind = match &s.kind {
            ParamKind::Float { min, max } => format!("float {}..{}", num(*min), num(*max)),
            ParamKind::Int { min, max } => format!("int {min}..{max}"),
            ParamKind::Bool => "bool".into(),
            ParamKind::Color => "color [r,g,b,a]".into(),
            ParamKind::Point => "point [x,y]".into(),
            ParamKind::Text => "text".into(),
            ParamKind::Choice { options } => format!("choice {}", options.join("|")),
            ParamKind::Image => "image path".into(),
        };
        out.line(
            i + 1,
            format!(
                "- {} ({}): {kind}, default {}",
                s.name,
                s.label(),
                value(&s.default)
            ),
        );
    }
}

fn audio_fx_uses(project: &Project, kind: AudioFxKind) -> Vec<String> {
    let tl = &project.timeline;
    let tracks = tl
        .tracks
        .iter()
        .filter(|t| t.mix.effects.iter().any(|e| e.kind == kind))
        .map(|t| format!("@track {}", t.id));
    let clips = tl.tracks.iter().flat_map(|t| &t.clips).flat_map(|c| {
        c.audio
            .effects
            .iter()
            .filter(|e| e.kind == kind)
            .map(move |e| format!("@effect {}/{}", c.id, e.id))
    });
    tracks.chain(clips).collect()
}

fn fx_label(project: &Project, r: &fx::FxRef) -> String {
    match fx::asset(project, r) {
        Some(a) => format!("@fx {r} \"{}\"", a.name),
        None => format!("@fx {r} (missing asset)"),
    }
}

fn list_or_none(items: &[String]) -> String {
    if items.is_empty() {
        "none".into()
    } else {
        items.join("; ")
    }
}

fn value(v: &ParamValue) -> String {
    match v {
        ParamValue::Bool(b) => b.to_string(),
        ParamValue::Number(n) => num(*n),
        ParamValue::List(l) => format!(
            "[{}]",
            l.iter().map(|n| num(*n)).collect::<Vec<_>>().join(", ")
        ),
        ParamValue::Text(t) => format!("{t:?}"),
    }
}

/// Up to three decimals, without trailing zeros.
fn num(n: f64) -> String {
    let s = format!("{n:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

fn secs(t: Time) -> String {
    format!("{:.3}", t.as_secs_f64())
}

fn range(r: TimeRange) -> String {
    format!("{}-{}", secs(r.start), secs(r.end))
}

fn stem(path: &std::path::Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The serde name of a unit enum variant (`start_on_cut`, `protected`…).
fn enum_name<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}
