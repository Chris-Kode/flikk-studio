//! PRD S5: import Markdown or plain-text notes as proposed style rules,
//! one per bullet or paragraph.

use crate::{Store, now};
use anyhow::{Context, Result};
use koo_core::ids::RuleId;
use koo_core::memory::{RuleScope, RuleStatus, StyleRule};
use std::path::Path;

impl Store {
    /// Read `path` and add its notes to `profile` as `Proposed` rules with
    /// the file name as evidence.
    pub fn import_notes(
        &self,
        profile: RuleId,
        path: &Path,
        scope: RuleScope,
    ) -> Result<Vec<StyleRule>> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        let source = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        self.import_notes_text(profile, &source, &text, scope)
    }

    pub fn import_notes_text(
        &self,
        profile: RuleId,
        source: &str,
        text: &str,
        scope: RuleScope,
    ) -> Result<Vec<StyleRule>> {
        let t = now();
        let rules: Vec<StyleRule> = parse_notes(text)
            .into_iter()
            .map(|text| StyleRule {
                id: RuleId::new(),
                text,
                scope: scope.clone(),
                status: RuleStatus::Proposed,
                supersedes: None,
                created_at: t,
                evidence: vec![source.to_owned()],
            })
            .collect();
        self.save_rules(profile, &rules)?;
        Ok(rules)
    }
}

/// Split notes into rule texts: each list item (with its wrapped lines) is
/// one rule, each other paragraph is one rule. Headings, code blocks, tables,
/// rules and front matter are skipped; inline Markdown is unwrapped.
pub fn parse_notes(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    let mut in_code = false;
    let mut lines = text.lines().peekable();

    // YAML front matter.
    if lines.peek().is_some_and(|l| l.trim() == "---") {
        lines.next();
        for l in lines.by_ref() {
            if l.trim() == "---" {
                break;
            }
        }
    }

    let flush = |current: &mut Option<String>, out: &mut Vec<String>| {
        if let Some(text) = current.take() {
            let text = clean(&text);
            if text.chars().any(char::is_alphanumeric) {
                out.push(text);
            }
        }
    };

    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            flush(&mut current, &mut out);
            in_code = !in_code;
            continue;
        }
        if in_code {
            continue;
        }
        if trimmed.is_empty() {
            flush(&mut current, &mut out);
            continue;
        }
        if trimmed.starts_with('#')
            || trimmed.starts_with('|')
            || is_thematic_break(trimmed)
            || is_setext_underline(trimmed)
        {
            flush(&mut current, &mut out);
            continue;
        }
        let unquoted = trimmed.trim_start_matches('>').trim_start();
        if let Some(item) = list_item(unquoted) {
            flush(&mut current, &mut out);
            current = Some(item.to_owned());
        } else if let Some(c) = current.as_mut() {
            c.push(' ');
            c.push_str(unquoted);
        } else {
            current = Some(unquoted.to_owned());
        }
    }
    flush(&mut current, &mut out);
    out
}

/// The text of a list item (`- x`, `* x`, `+ x`, `1. x`, `1) x`, `- [ ] x`).
fn list_item(line: &str) -> Option<&str> {
    let rest = if let Some(r) = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .or_else(|| line.strip_prefix("+ "))
        .or_else(|| line.strip_prefix("• "))
    {
        r
    } else {
        let digits = line.chars().take_while(char::is_ascii_digit).count();
        if digits == 0 || digits > 3 {
            return None;
        }
        let r = &line[digits..];
        r.strip_prefix(". ").or_else(|| r.strip_prefix(") "))?
    };
    let rest = rest.trim_start();
    let rest = ["[ ] ", "[x] ", "[X] "]
        .iter()
        .find_map(|b| rest.strip_prefix(b))
        .unwrap_or(rest);
    Some(rest.trim())
}

fn is_thematic_break(line: &str) -> bool {
    let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    compact.len() >= 3
        && ['-', '*', '_']
            .iter()
            .any(|m| compact.chars().all(|c| c == *m))
}

fn is_setext_underline(line: &str) -> bool {
    line.len() >= 2 && line.chars().all(|c| c == '=')
}

/// Unwrap emphasis, code spans and links; collapse whitespace.
fn clean(text: &str) -> String {
    let mut s = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' | '`' => {}
            '_' if s.is_empty()
                || s.ends_with(' ')
                || chars.peek().is_none_or(|n| !n.is_alphanumeric()) => {}
            '!' if chars.peek() == Some(&'[') => {}
            '[' => {}
            ']' if chars.peek() == Some(&'(') => {
                // Drop the link target.
                let mut depth = 0;
                for n in chars.by_ref() {
                    match n {
                        '(' => depth += 1,
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            }
            ']' => {}
            _ => s.push(c),
        }
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}
