//! Tool arguments at the boundary. Times are seconds (`f64`), `"M:SS.mmm"`,
//! timecode `"HH:MM:SS:FF"` or frames `"f:1234"` at the sequence rate (and
//! `"@time 12.5"` / `"@range 1.000-2.000"` chat tokens). Ids are the 8-hex
//! short form shown in results, a full UUID, or a chat reference token such
//! as `@clip 3f2a91c0`.

use koo_core::model::{Project, Track, TrackKind};
use koo_core::{ClipId, MarkerId, MediaId, Rate, SpeakerId, Time, TimeRange, TrackId};
use serde_json::{Map, Value, json};
use std::fmt::Display;
use uuid::Uuid;

/// A tool failure the model can read and act on.
#[derive(Debug, Clone)]
pub struct ToolError(pub String);

impl<E: Display> From<E> for ToolError {
    fn from(e: E) -> Self {
        ToolError(e.to_string())
    }
}

pub type ToolResultOf<T> = std::result::Result<T, ToolError>;

pub fn fail<T>(message: impl Into<String>) -> ToolResultOf<T> {
    Err(ToolError(message.into()))
}

/// Tool arguments with typed accessors, and the sequence rate that
/// timecodes and frame counts are read at.
pub struct Args<'a>(pub &'a Map<String, Value>, pub Rate);

impl<'a> Args<'a> {
    pub fn of(value: &'a Value, rate: Rate) -> ToolResultOf<Self> {
        match value.as_object() {
            Some(m) => Ok(Args(m, rate)),
            None => fail("arguments must be an object"),
        }
    }

    /// A nested object under `key`, read at the same rate.
    pub fn object(&self, key: &str) -> ToolResultOf<Option<Args<'a>>> {
        match self.get(key) {
            None => Ok(None),
            Some(Value::Object(m)) => Ok(Some(Args(m, self.1))),
            Some(_) => fail(format!("`{key}` must be an object")),
        }
    }

    pub fn value(&self, key: &str) -> Option<&'a Value> {
        self.get(key)
    }

    fn get(&self, key: &str) -> Option<&'a Value> {
        self.0.get(key).filter(|v| !v.is_null())
    }

    pub fn has(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    pub fn str(&self, key: &str) -> ToolResultOf<&'a str> {
        self.opt_str(key)?
            .ok_or_else(|| ToolError(format!("`{key}` is required")))
    }

    pub fn opt_str(&self, key: &str) -> ToolResultOf<Option<&'a str>> {
        match self.get(key) {
            None => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.as_str())),
            Some(_) => fail(format!("`{key}` must be a string")),
        }
    }

    pub fn f64(&self, key: &str) -> ToolResultOf<f64> {
        self.opt_f64(key)?
            .ok_or_else(|| ToolError(format!("`{key}` is required")))
    }

    pub fn opt_f64(&self, key: &str) -> ToolResultOf<Option<f64>> {
        match self.get(key) {
            None => Ok(None),
            Some(v) => v
                .as_f64()
                .filter(|f| f.is_finite())
                .map(Some)
                .ok_or_else(|| ToolError(format!("`{key}` must be a number"))),
        }
    }

    pub fn opt_bool(&self, key: &str) -> ToolResultOf<Option<bool>> {
        match self.get(key) {
            None => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(_) => fail(format!("`{key}` must be true or false")),
        }
    }

    pub fn strings(&self, key: &str) -> ToolResultOf<Vec<String>> {
        match self.get(key) {
            None => Ok(Vec::new()),
            Some(Value::String(s)) => Ok(vec![s.clone()]),
            Some(Value::Array(items)) => items
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| ToolError(format!("`{key}` must be a list of strings")))
                })
                .collect(),
            Some(_) => fail(format!("`{key}` must be a list of strings")),
        }
    }

    pub fn time(&self, key: &str) -> ToolResultOf<Time> {
        self.opt_time(key)?
            .ok_or_else(|| ToolError(format!("`{key}` is required")))
    }

    pub fn opt_time(&self, key: &str) -> ToolResultOf<Option<Time>> {
        self.get(key)
            .map(|v| parse_time(v, self.1).map_err(|e| ToolError(format!("`{key}`: {}", e.0))))
            .transpose()
    }

    /// A length in any time form; negative allowed (deltas).
    pub fn opt_duration(&self, key: &str) -> ToolResultOf<Option<Time>> {
        self.opt_time(key)
    }

    pub fn duration(&self, key: &str) -> ToolResultOf<Time> {
        self.time(key)
    }

    /// `{"start": t, "end": t}` under `key`, or `None` when absent.
    pub fn opt_range(&self, key: &str) -> ToolResultOf<Option<TimeRange>> {
        let Some(v) = self.get(key) else {
            return Ok(None);
        };
        let range = match v {
            Value::String(s) => range_token(s, self.1)
                .ok_or_else(|| ToolError(format!("`{key}` must be {{start, end}}")))?,
            _ => {
                let r = Args::of(v, self.1)
                    .map_err(|_| ToolError(format!("`{key}` must be {{start, end}}")))?;
                TimeRange::new(r.time("start")?, r.time("end")?)
            }
        };
        if range.is_empty() {
            return fail(format!("`{key}` is empty: end must be after start"));
        }
        Ok(Some(range))
    }

    /// The range under `key`, defaulting to the whole timeline.
    pub fn range_or_all(&self, key: &str, project: &Project) -> ToolResultOf<TimeRange> {
        Ok(self
            .opt_range(key)?
            .unwrap_or_else(|| TimeRange::new(Time::ZERO, project.timeline.duration())))
    }

    pub fn range(&self, key: &str) -> ToolResultOf<TimeRange> {
        self.opt_range(key)?
            .ok_or_else(|| ToolError(format!("`{key}` is required")))
    }
}

pub fn parse_time(v: &Value, rate: Rate) -> ToolResultOf<Time> {
    let t = match v {
        Value::Number(n) => n
            .as_f64()
            .filter(|f| f.is_finite())
            .map(Time::from_secs_f64),
        Value::String(s) => time_text(s, rate),
        _ => None,
    };
    if let (None, Value::String(s)) = (t, v)
        && s.split(':').count() == 4
    {
        return fail(format!(
            "`{s}` is not a timecode at {} fps: HH:MM:SS:FF with FF 0..{}",
            rate.rounded_fps(),
            rate.rounded_fps().saturating_sub(1)
        ));
    }
    t.ok_or_else(|| {
        ToolError(
            "times are seconds, \"M:SS.mmm\", timecode \"HH:MM:SS:FF\" or frames \"f:1234\"".into(),
        )
    })
}

/// `12.5`, `1:02.500`, `1:02:03.5`, `00:01:02:15` (timecode at `rate`),
/// `f:1234` (frames at `rate`), `@time 12.5`; a leading `-` negates.
pub fn time_text(text: &str, rate: Rate) -> Option<Time> {
    let mut s = text.trim();
    if let Some(rest) = s.strip_prefix("@time") {
        s = rest.trim();
    }
    let (neg, s) = match s.strip_prefix('-') {
        Some(rest) => (true, rest.trim()),
        None => (false, s),
    };
    let t = if let Some(frames) = s.strip_prefix("f:").or_else(|| s.strip_prefix("F:")) {
        Time::from_frame(frames.trim().parse::<i64>().ok()?, rate)
    } else if s.split(':').count() == 4 {
        let parts: Vec<i64> = s
            .split(':')
            .map(|p| p.trim().parse::<i64>().ok())
            .collect::<Option<_>>()?;
        let fps = rate.rounded_fps() as i64;
        if parts[3] >= fps || parts[1] >= 60 || parts[2] >= 60 {
            return None;
        }
        let frames = ((parts[0] * 60 + parts[1]) * 60 + parts[2]) * fps + parts[3];
        Time::from_frame(frames, rate)
    } else {
        Time::parse_clock(s)?
    };
    Some(if neg { -t } else { t })
}

/// `@range 1.000-2.000` (or the same without the `@range`), any time form
/// on either side of the dash.
pub fn range_token(text: &str, rate: Rate) -> Option<TimeRange> {
    let s = text.trim();
    let s = s.strip_prefix("@range").map(str::trim).unwrap_or(s);
    // Split on the dash between the two times, not a sign.
    let ix = s
        .char_indices()
        .skip(1)
        .find(|(_, c)| *c == '-' || *c == '–')
        .map(|(i, _)| i)?;
    let dash = s[ix..].chars().next()?.len_utf8();
    let range = TimeRange::new(
        time_text(&s[..ix], rate)?,
        time_text(&s[ix + dash..], rate)?,
    );
    (!range.is_empty()).then_some(range)
}

/// Seconds rounded to the millisecond, the form results use.
pub fn secs(t: Time) -> Value {
    json!((t.as_millis() as f64) / 1000.0)
}

pub fn range_json(r: TimeRange) -> Value {
    json!({ "start": secs(r.start), "end": secs(r.end) })
}

pub fn short(id: impl Display) -> String {
    id.to_string()
}

/// The id part of a chat reference token: `@clip 3f2a91c0` → `3f2a91c0`.
/// Other text is returned trimmed.
pub fn strip_token(text: &str) -> &str {
    let t = text.trim();
    match t.strip_prefix('@') {
        Some(rest) => match rest.split_once(char::is_whitespace) {
            Some((_, id)) => id.trim(),
            None => rest,
        },
        None => t,
    }
}

fn matches_id(uuid: &Uuid, text: &str) -> bool {
    let text = strip_token(text).to_ascii_lowercase();
    if text.len() < 4 {
        return false;
    }
    let simple = uuid.simple().to_string();
    simple.starts_with(&text) || uuid.hyphenated().to_string() == text
}

fn pick<T: Copy>(kind: &str, text: &str, found: Vec<T>) -> ToolResultOf<T> {
    match found.as_slice() {
        [one] => Ok(*one),
        [] => fail(format!("no {kind} with id `{text}`")),
        _ => fail(format!("`{text}` matches several {kind}s; use more digits")),
    }
}

/// A clip by id; `@effect <clip>/<effect>` and `@transition <clip>/in`
/// tokens name their clip too.
pub fn clip_id(project: &Project, text: &str) -> ToolResultOf<ClipId> {
    let text = strip_token(text).split('/').next().unwrap_or_default();
    let found = project
        .timeline
        .tracks
        .iter()
        .flat_map(|t| &t.clips)
        .filter(|c| matches_id(&c.id.0, text))
        .map(|c| c.id)
        .collect();
    pick("clip", text, found)
}

pub fn clip_ids(project: &Project, texts: &[String]) -> ToolResultOf<Vec<ClipId>> {
    texts.iter().map(|t| clip_id(project, t)).collect()
}

pub fn media_id(project: &Project, text: &str) -> ToolResultOf<MediaId> {
    let found = project
        .media
        .iter()
        .filter(|m| {
            matches_id(&m.id.0, text)
                || m.path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy() == strip_token(text))
        })
        .map(|m| m.id)
        .collect();
    pick("media item", text, found)
}

/// By id or by name ("V1").
pub fn track_id(project: &Project, text: &str) -> ToolResultOf<TrackId> {
    let tracks = &project.timeline.tracks;
    let text = strip_token(text);
    let by_name: Vec<_> = tracks
        .iter()
        .filter(|t| t.name.eq_ignore_ascii_case(text))
        .map(|t| t.id)
        .collect();
    if !by_name.is_empty() {
        return pick("track", text, by_name);
    }
    let found = tracks
        .iter()
        .filter(|t| matches_id(&t.id.0, text))
        .map(|t| t.id)
        .collect();
    pick("track", text, found)
}

pub fn marker_id(project: &Project, text: &str) -> ToolResultOf<MarkerId> {
    let found = project
        .timeline
        .markers
        .iter()
        .filter(|m| matches_id(&m.id.0, text))
        .map(|m| m.id)
        .collect();
    pick("marker", text, found)
}

/// Speakers are only known from analysis, so the caller passes candidates.
pub fn speaker_id(candidates: &[SpeakerId], text: &str) -> ToolResultOf<SpeakerId> {
    let found = candidates
        .iter()
        .filter(|s| matches_id(&s.0, text))
        .copied()
        .collect();
    pick("speaker", text, found)
}

/// `tracks` argument → `TrackScope` (`None` = every unlocked track).
pub fn track_scope(project: &Project, args: &Args) -> ToolResultOf<Option<Vec<TrackId>>> {
    let names = args.strings("tracks")?;
    if names.is_empty() {
        return Ok(None);
    }
    names
        .iter()
        .map(|n| track_id(project, n))
        .collect::<ToolResultOf<Vec<_>>>()
        .map(Some)
}

pub fn first_track(project: &Project, kind: TrackKind) -> Option<&Track> {
    project.timeline.tracks.iter().find(|t| t.kind == kind)
}

pub fn kind_name(kind: TrackKind) -> &'static str {
    match kind {
        TrackKind::Video => "video",
        TrackKind::Audio => "audio",
        TrackKind::Title => "title",
        TrackKind::Caption => "caption",
        TrackKind::Overlay => "overlay",
    }
}
