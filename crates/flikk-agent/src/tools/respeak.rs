//! `replace_words` (the Generate group): words in the transcript said
//! again as other words, in the speaker's own voice, and cut in where they
//! were (`flikk_core::respeak`). The voice is the one the user linked to that
//! speaker (by fixing a word of theirs in the transcript, which clones it
//! with their consent), or one the agent names from `find_voices`. It costs
//! credits, so it waits for the user's approval unless they allowed
//! generating; the new words land in the Voiceover Generated bin and the
//! cut is proposed in the chat's change set.

use super::{Done, Job, JobDone, Step, ToolCtx};
use crate::args::{Args, ToolError, ToolResultOf, fail, short};
use flikk_core::bus::{Author, CommandBus};
use flikk_core::cloud::{Capability, Reading};
use flikk_core::library::Library;
use flikk_core::model::MediaItem;
use flikk_core::query::{self, TimelineWord};
use flikk_core::respeak::{self, Fix};
use flikk_core::{MediaId, Time, TimeRange};
use serde_json::{Value, json};

/// The fix and the voice it is said in, from the tool's arguments.
pub fn parse(ctx: &ToolCtx, args: &Args) -> ToolResultOf<(Fix, Reading)> {
    let Some(analysis) = ctx.services.analysis.clone() else {
        return fail("Analysis is not available in this build yet.");
    };
    let Some(generation) = ctx.services.generation.clone() else {
        return fail("Generation is not available in this build.");
    };
    let Some(provider) = generation
        .provider_for(Capability::Voiceover)
        .and(generation.voice_provider())
    else {
        return fail(super::generate::unavailable_for(Capability::Voiceover));
    };
    let project = ctx.project();
    let all = TimeRange::new(Time::ZERO, project.timeline.duration());
    let mut words = query::words_in_range(project, all, |fp| analysis.analysis(fp));
    // As the transcript shows them.
    words.retain(|w| {
        !w.word.text.trim().is_empty() && w.timeline.duration().0 * 2 >= w.source.duration().0
    });
    let near = |t: Time| -> ToolResultOf<usize> {
        let k = words
            .iter()
            .enumerate()
            .min_by_key(|(_, w)| (w.timeline.start - t).0.abs())
            .map(|(k, _)| k)
            .ok_or_else(|| ToolError("there is no transcribed speech on the timeline".into()))?;
        if (words[k].timeline.start - t).0.abs() > Time::from_millis(250).0 {
            return fail(format!(
                "no word starts at {t}; the nearest is \"{}\" at {}",
                words[k].word.text.trim(),
                words[k].timeline.start
            ));
        }
        Ok(k)
    };
    let first = near(args.time("from")?)?;
    let last = match args.opt_time("to")? {
        Some(t) => near(t)?,
        None => first,
    };
    if last < first {
        return fail("`to` is before `from`");
    }
    let said = said(&words[first..=last]);
    if let Some(expected) = args.opt_str("words")?
        && normalized(expected) != normalized(&said)
    {
        return fail(format!(
            "the words from {} to {} are \"{said}\", not \"{expected}\"",
            words[first].timeline.start, words[last].timeline.start
        ));
    }
    let text = args.str("text")?;
    let fix = respeak::fix(project, &words, first, last, text, |fp| {
        analysis.analysis(fp)
    })
    .map_err(ToolError)?;
    let lib = Library::load();
    let reading = match args
        .opt_str("voice")?
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        Some(voice) => match lib.voice_of(&provider, voice) {
            Some(saved) => saved.reading(),
            None => Reading {
                voice: voice.to_owned(),
                voice_name: args.opt_str("voice_name")?.unwrap_or(voice).to_owned(),
                stability: Reading::STABILITY,
                similarity: Reading::SIMILARITY,
            },
        },
        None => match fix
            .speaker()
            .and_then(|who| lib.voice_of_speaker(&provider, &who))
        {
            Some(saved) => saved.reading(),
            None => {
                return fail(
                    "No voice is linked to this speaker yet. Cloning one needs their consent: ask the user to retype the words in the transcript (select them, Retype…, then say it in their voice), which clones it once and links it; or pass a `voice` from find_voices that sounds like them.",
                );
            }
        },
    };
    Ok((fix, reading))
}

fn said(words: &[TimelineWord]) -> String {
    words
        .iter()
        .map(|w| w.word.text.trim())
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalized(text: &str) -> String {
    text.split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// One line for the approval card and the activity log.
pub fn summary(fix: &Fix, reading: &Reading) -> String {
    format!(
        "Say {} in {}’s voice ({:.1}s)",
        fix.summary(),
        reading.voice_name,
        fix.range.start.as_secs_f64()
    )
}

/// The fix with its voice, as an approval carries it.
pub fn request_json(fix: &Fix, reading: &Reading) -> Value {
    json!({ "fix": fix, "reading": reading })
}

/// What [`request_json`] made.
pub fn from_json(value: &Value) -> Option<(Fix, Reading)> {
    Some((
        serde_json::from_value(value.get("fix")?.clone()).ok()?,
        serde_json::from_value(value.get("reading")?.clone()).ok()?,
    ))
}

/// With the user's leave: a job that has the words said and fitted, then
/// comes back to the UI thread to add them to the pool and propose the cut.
pub fn serve(ctx: &mut ToolCtx, args: &Args) -> ToolResultOf<Step> {
    let (fix, reading) = parse(ctx, args)?;
    let Some(source) = ctx.project().media(fix.media).cloned() else {
        return fail("the recording is not in the pool");
    };
    let services = ctx.services.clone();
    let task = ctx.stop.task();
    let job: Job = Box::new(move || {
        let summary = summary(&fix, &reading);
        match respeak::make(&fix, &source, &reading, &services, &task) {
            Ok(item) => JobDone::Respoken {
                item: Box::new(item),
                fix: Box::new(fix),
                summary,
            },
            Err(e) => JobDone::Done(Done::error(e)),
        }
    });
    Ok(Step::Job(job))
}

/// Back on the UI thread with the new words: add them to the pool, and
/// propose them in place of the old as `author` (none when the turn was
/// cancelled).
pub fn finish(
    bus: &mut CommandBus,
    author: Option<&Author>,
    item: MediaItem,
    fix: &Fix,
    summary: &str,
) -> Done {
    let media = match super::generate::add_to_pool(bus, item, author) {
        Ok(media) => media,
        Err(e) => return Done::error(format!("Made it, but could not add it to the pool: {e}")),
    };
    let mut text = format!(
        "Made it: media {} in the Voiceover Generated bin.",
        short(media)
    );
    match author {
        Some(author) => match propose(bus, author, media, fix, summary) {
            Ok(placed) => {
                text.push(' ');
                text.push_str(&placed);
            }
            Err(e) => text.push_str(&format!(" Could not cut it in: {e}")),
        },
        None => text.push_str(" The turn was cancelled, so it was not cut in."),
    }
    Done::text(format!("fixed {}", fix.summary()), text)
}

/// Propose `media`, the new words of `fix`, in place of the old ones as
/// `author`. What happened, for the model.
pub fn propose(
    bus: &mut CommandBus,
    author: &Author,
    media: MediaId,
    fix: &Fix,
    reason: &str,
) -> Result<String, String> {
    let command = fix.command(bus.view_for(author), media)?;
    let range = match &command {
        flikk_core::command::Command::ReplaceSpeech { range, .. } => *range,
        _ => fix.range,
    };
    bus.execute(command, author.clone(), reason)
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "Proposed the new words in place of “{}” at {:.3}–{:.3} s; what follows moves with them.",
        fix.old,
        range.start.as_secs_f64(),
        range.end.as_secs_f64()
    ))
}
