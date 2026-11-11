//! `check_timeline`: the mistakes a viewer notices, as
//! `flikk_core::check` finds them, for the whole timeline or a range. The
//! same findings a turn's change set brings in reach the agent unasked:
//! [`set_findings`] works them out after each call that grew the set, and
//! the host adds them to that call's result, so the last result of the
//! turn always says what the set still brings in.

use super::{Done, Job, JobDone, ToolCtx};
use crate::args::*;
use crate::bridge::Content;
use flikk_core::bus::{Author, CommandBus};
use flikk_core::check::{self, Finding, Report, Sources};
use flikk_core::model::Project;
use flikk_core::services::Services;
use serde_json::{Value, json};
use std::sync::Arc;

pub fn check_timeline(ctx: &mut ToolCtx, args: &Args) -> Result<super::Step, ToolError> {
    let project = Arc::new(ctx.project().clone());
    let range = args.opt_range("range")?;
    let services = ctx.services.clone();
    let job: Job = Box::new(move || {
        let sources = Sources::on_disk(&project, &services);
        let report = check::check(&project, range, &sources);
        let summary = match report.findings.len() {
            0 => "no findings".to_owned(),
            1 => "1 finding".to_owned(),
            n => format!("{n} findings"),
        };
        JobDone::Done(Done::json(summary, &report_json(&report)))
    });
    Ok(super::Step::Job(job))
}

/// A report as the tool returns it.
fn report_json(report: &Report) -> Value {
    let mut out = json!({ "findings": findings_json(&report.findings) });
    if !report.skipped.is_empty() {
        out["skipped"] = json!(report.skipped.iter().map(|k| k.name()).collect::<Vec<_>>());
        out["skipped_note"] =
            json!("These checks left out media that is not analyzed yet (or no lettering engine)");
    }
    out
}

/// Findings as tools return them: kind, where, the clips and a note.
pub(crate) fn findings_json(findings: &[Finding]) -> Value {
    findings
        .iter()
        .map(|f| {
            let mut v = json!({
                "kind": f.kind.name(),
                "at": secs(f.range.start),
                "clips": f.clips.iter().map(|c| short(*c)).collect::<Vec<_>>(),
                "note": f.note,
            });
            if !f.range.is_empty() {
                v["end"] = json!(secs(f.range.end));
            }
            if f.severity == check::Severity::Notice {
                v["severity"] = json!("notice");
            }
            v
        })
        .collect()
}

/// The open change set of `author` checked against the project, off the
/// UI thread: a job giving what the set brings in. `None` when `author`
/// has no open set with items.
pub(crate) fn set_findings(
    bus: &CommandBus,
    author: &Author,
    services: &Services,
) -> Option<impl FnOnce() -> Vec<Finding> + Send + use<>> {
    let set = bus
        .change_sets()
        .iter()
        .find(|s| &s.author == author && s.state == flikk_core::changeset::ChangeSetState::Open)?;
    let after: Project = set.draft.clone().filter(|_| !set.items.is_empty())?;
    let before = bus.project().clone();
    let services = services.clone();
    Some(move || {
        let sources = Sources::on_disk(&after, &services);
        let was = check::check(&before, None, &sources);
        let now = check::check(&after, None, &sources);
        check::introduced(&was.findings, &now.findings)
    })
}

/// `result` of a call that changed the change set, with what the set now
/// brings in when that differs from what the agent was last told
/// (`told`, kept up to date).
pub(crate) fn tell(done: &mut Done, found: Vec<Finding>, told: &mut Vec<Finding>) {
    let same = found.len() == told.len()
        && found
            .iter()
            .zip(told.iter())
            .all(|(a, b)| a.kind == b.kind && a.clips == b.clips && a.range == b.range);
    if same {
        return;
    }
    let text = if found.is_empty() {
        "\n\nCheck: the change set no longer brings in any mistakes.".to_owned()
    } else {
        format!(
            "\n\nCheck: the change set brings in {} (fix each, or say why it is meant, before you finish): {}",
            match found.len() {
                1 => "1 mistake".to_owned(),
                n => format!("{n} mistakes"),
            },
            findings_json(&found)
        )
    };
    match done.result.content.first_mut() {
        Some(Content::Text { text: first }) => first.push_str(&text),
        _ => done.result.content.push(Content::Text {
            text: text.trim_start().to_owned(),
        }),
    }
    *told = found;
}
