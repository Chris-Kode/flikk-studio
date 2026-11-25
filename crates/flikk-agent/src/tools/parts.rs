//! Long text for the agent, in parts: a call returns the lines that fit a
//! size budget and says how to ask for the next ones, so no result is too
//! big for the CLI that carries it. Claude Code moves a text result of more
//! than 50,000 characters into a file, which the agent has no tool to open
//! here, and Codex cuts one that is past its token budget.

use crate::args::{Args, ToolResultOf};
use std::fmt::Write as _;

/// Lines in a part unless the call asks for fewer.
const LINES_DEFAULT: usize = 2000;
/// Most text in a part, in bytes: about 8,000 tokens of code.
const BUDGET: usize = 28 << 10;

/// The lines a call asked for: `offset`, the first one counted from 1
/// (default 1), and `limit`, how many (default 2000).
pub(crate) struct Window {
    first: usize,
    lines: usize,
}

impl Window {
    pub(crate) fn of(args: &Args) -> ToolResultOf<Self> {
        let count = |key| args.opt_f64(key).map(|n| n.map(|n| n.max(1.0) as usize));
        Ok(Self {
            first: count("offset")?.unwrap_or(1),
            lines: count("limit")?.unwrap_or(LINES_DEFAULT),
        })
    }
}

/// The lines of `text` in `window`, each behind its number: for code, where
/// errors and later reads go by line. Ends with a note giving the offset of
/// the next part when there is one.
pub(crate) fn numbered(text: &str, window: &Window) -> String {
    part(text, window, |n| format!("{n:>6}\t"))
}

/// As [`numbered`] without the numbers: for prose the agent writes back as
/// it read it.
pub(crate) fn plain(text: &str, window: &Window) -> String {
    part(text, window, |_| String::new())
}

fn part(text: &str, window: &Window, prefix: impl Fn(usize) -> String) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    if total == 0 {
        return "(empty)\n".into();
    }
    let first = window.first.min(total);
    let mut out = String::new();
    let mut last = first - 1;
    for (i, line) in lines.iter().enumerate().skip(first - 1).take(window.lines) {
        let n = i + 1;
        if out.len() + line.len() <= BUDGET {
            let _ = writeln!(out, "{}{line}", prefix(n));
        } else if out.is_empty() {
            // A line bigger than a whole part (minified code, an embedded
            // picture) is cut: left out, the same offset would be asked for
            // again and again.
            let _ = writeln!(
                out,
                "{}{}\n(line {n} is {} characters; the rest of it is left out)",
                prefix(n),
                clipped(line, BUDGET),
                line.chars().count()
            );
        } else {
            break;
        }
        last = n;
    }
    if last < total {
        let _ = writeln!(
            out,
            "(lines {first}–{last} of {total}; pass offset {} for more)",
            last + 1
        );
    }
    out
}

/// `line`, or its first `width` characters and an ellipsis when it is
/// longer.
pub(crate) fn clipped(line: &str, width: usize) -> String {
    match line.char_indices().nth(width) {
        Some((end, _)) => format!("{}…", &line[..end]),
        None => line.to_owned(),
    }
}
