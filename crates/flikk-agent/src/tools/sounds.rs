//! Sounds to use: `find_sounds` searches the user's library and this
//! project's pool, and `use_library_sound` adds a library one to the pool
//! and proposes it on an audio track. `save_sound_to_library` goes through
//! approval.

use super::audio::{MIN_SCORE, round};
use super::generate::{Placement, propose_placement};
use super::{Done, Job, JobDone, Step, ToolCtx};
use crate::args::{Args, ToolError, range_json, secs, short, strip_token, track_id};
use flikk_core::cloud::GenerationInfo;
use flikk_core::library::{self, Library, sound_category};
use flikk_core::model::{Fingerprint, MediaKind, Project, TrackKind};
use flikk_core::{Time, TimeRange};
use serde_json::{Value, json};

/// A sound to look through.
struct Candidate {
    value: Value,
    /// Lower case: name, category, tags and what a provider was asked.
    words: String,
    fingerprint: Option<Fingerprint>,
    /// Share of the query's words it carries.
    by_words: f32,
    /// Its best window for the query, in source time, and the score.
    heard: Option<(TimeRange, f32)>,
}

impl Candidate {
    fn new(
        mut value: Value,
        name: &str,
        duration: Option<Time>,
        tags: &[String],
        generated: Option<&GenerationInfo>,
        fingerprint: Option<&Fingerprint>,
    ) -> Self {
        let category = sound_category(generated);
        value["name"] = json!(name);
        value["category"] = json!(category);
        if let Some(duration) = duration {
            value["duration"] = secs(duration);
        }
        if !tags.is_empty() {
            value["tags"] = json!(tags);
        }
        let prompt = generated.map_or("", |g| g.prompt.as_str());
        let mut words = format!("{name} {category} {prompt}");
        for tag in tags {
            words.push(' ');
            words.push_str(tag);
        }
        Self {
            value,
            words: words.to_lowercase(),
            fingerprint: fingerprint.cloned(),
            by_words: 0.0,
            heard: None,
        }
    }

    fn rank(&self) -> f32 {
        self.by_words + self.heard.map_or(0.0, |h| h.1)
    }
}

/// The audio to search: the library's, then the pool's that isn't in it.
fn candidates(project: &Project, lib: &Library, scope: &str) -> Vec<Candidate> {
    let mut all = Vec::new();
    if scope != "project" {
        for s in &lib.sounds {
            let mut value = json!({ "source": "library", "library_id": s.id.to_string() });
            let (media, added) = library::sound_media(project, s);
            if added.is_empty() {
                value["media"] = json!(short(media));
            }
            all.push(Candidate::new(
                value,
                &s.name,
                s.duration(),
                &s.tags,
                s.generated.as_deref(),
                s.fingerprint.as_ref(),
            ));
        }
    }
    if scope != "library" {
        let audio = project.media.iter().filter(|m| m.kind == MediaKind::Audio);
        for m in audio.filter(|m| scope == "project" || lib.sound_of(m).is_none()) {
            let name = m.path.file_name().unwrap_or_default().to_string_lossy();
            all.push(Candidate::new(
                json!({ "source": "project", "media": short(m.id) }),
                &name,
                m.info.as_ref().map(|i| i.duration),
                &m.tags,
                m.generated.as_deref(),
                m.fingerprint.as_ref(),
            ));
        }
    }
    all
}

/// Music and sound effects to use, in the library and the pool: by the
/// words they carry and, where analysis left vectors, by how they sound.
pub fn find(ctx: &mut ToolCtx, args: &Args) -> Result<Step, ToolError> {
    let text = args
        .opt_str("text")?
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_owned);
    let scope = args.opt_str("scope")?.unwrap_or("all");
    if !["all", "library", "project"].contains(&scope) {
        return Err(ToolError("`scope` is all, library or project".to_owned()));
    }
    let limit = args.opt_f64("limit")?.unwrap_or(12.0).clamp(1.0, 50.0) as usize;
    let all = candidates(ctx.project(), &Library::load(), scope);
    let service = ctx.services.analysis.clone();
    let job: Job = Box::new(move || {
        let Some(text) = text else {
            let total = all.len();
            let sounds: Vec<Value> = all.into_iter().take(limit).map(|c| c.value).collect();
            let summary = format!("{total} sounds");
            return JobDone::Done(Done::json(
                summary,
                &json!({ "sounds": sounds, "total": total }),
            ));
        };
        let query = service.as_ref().and_then(|s| s.describe_sound(&text).ok());
        let words: Vec<String> = text
            .to_lowercase()
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        let mut all = all;
        for c in &mut all {
            let carried = words.iter().filter(|w| c.words.contains(w.as_str()));
            c.by_words = carried.count() as f32 / words.len() as f32;
            let vectors = c
                .fingerprint
                .as_ref()
                .and_then(|f| service.as_ref().and_then(|s| s.sound_vectors(f)));
            c.heard = vectors.zip(query.as_ref()).and_then(|(v, q)| {
                let scores = v.scores(q);
                let best = scores.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1));
                best.map(|(i, score)| (v.range(i), *score))
            });
        }
        let unheard = all.iter().filter(|c| c.heard.is_none()).count();
        // Weak matches are noise, as in search_sounds.
        let best = all
            .iter()
            .filter_map(|c| c.heard.map(|h| h.1))
            .fold(0.0, f32::max);
        let floor = MIN_SCORE.max(best / 2.0);
        for c in &mut all {
            c.heard = c.heard.filter(|h| h.1 >= floor);
        }
        all.retain(|c| c.by_words > 0.0 || c.heard.is_some());
        all.sort_by(|a, b| b.rank().total_cmp(&a.rank()));
        let total = all.len();
        let sounds: Vec<Value> = all
            .into_iter()
            .take(limit)
            .map(|c| {
                let mut v = c.value;
                if let Some((range, score)) = c.heard {
                    v["score"] = json!(round(score as f64, 2));
                    v["heard_at"] = range_json(range);
                }
                v
            })
            .collect();
        let mut out = json!({ "query": text, "sounds": sounds, "total": total });
        if unheard > 0 {
            out["note"] = json!(format!(
                "{unheard} searched by words only: how audio sounds is searched once it has \
                 been analysed."
            ));
        }
        JobDone::Done(Done::json(format!("sounds {text:?}: {total}"), &out))
    });
    Ok(Step::Job(job))
}

/// Add a sound to the pool as the user's edit, like generated audio, and
/// propose it where asked.
pub fn use_sound(ctx: &mut ToolCtx, args: &Args) -> Result<Step, ToolError> {
    let lib = Library::load();
    let text = strip_token(args.str("sound")?).trim().to_lowercase();
    let sound = lib
        .sounds
        .iter()
        .find(|s| s.id.to_string() == text || s.id.0.to_string() == text)
        .or_else(|| lib.sounds.iter().find(|s| s.name.to_lowercase() == text))
        .ok_or_else(|| ToolError(format!("no library sound `{text}`; find_sounds lists them")))?;
    let project = ctx.project();
    let track = args
        .opt_str("track")?
        .map(|t| track_id(project, t))
        .transpose()?;
    if let Some(track) = track
        && project
            .timeline
            .track(track)
            .is_some_and(|t| t.kind != TrackKind::Audio)
    {
        return Err(ToolError("`track` must be an audio track".into()));
    }
    let placement = args.opt_time("at")?.map(|at| Placement { at, track });
    let reason = args
        .opt_str("reason")?
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .unwrap_or("use_library_sound")
        .to_owned();
    let (media, commands) = library::sound_media(ctx.bus.project(), sound);
    if !commands.is_empty() {
        ctx.bus
            .execute_shared(commands, "Add sound")
            .map_err(|e| ToolError(e.to_string()))?;
    }
    let mut text = format!("In the pool as media {}.", short(media));
    if let Some(placement) = placement {
        let placed = propose_placement(ctx.bus, &ctx.author, media, &placement, &reason)
            .map_err(ToolError)?;
        text.push(' ');
        text.push_str(&placed);
    }
    Ok(Done::text(format!("sound {}", sound.name), text).into())
}
