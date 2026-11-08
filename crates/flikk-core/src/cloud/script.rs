//! A voiceover's script: what a voice can read, and the words it says.

use super::{CloudError, CloudResult, SCRIPT_CHARS};

/// Why `text` can't be read as a voiceover; the rewrite that can, when
/// there is one.
pub fn check_script(text: &str) -> CloudResult<()> {
    let refuse = |message: String, suggestion| {
        Err(CloudError::BadPrompt {
            message,
            suggestion,
        })
    };
    let chars = text.chars().count();
    if chars > SCRIPT_CHARS {
        return refuse(
            format!("A voiceover says at most {SCRIPT_CHARS} characters; this has {chars}"),
            None,
        );
    }
    match without_breaks(text) {
        Some(fixed) => refuse("<break> tags aren't read; use pauses".into(), Some(fixed)),
        None => Ok(()),
    }
}

/// `text` with its SSML `<break/>` tags as the pauses a voice reads; `None`
/// when it has none.
pub fn without_breaks(text: &str) -> Option<String> {
    // Lower case keeps every byte where it is.
    let lower = text.to_ascii_lowercase();
    let mut out = String::new();
    let mut at = 0;
    while let Some(start) = lower[at..].find("<break").map(|i| at + i) {
        let Some(end) = lower[start..].find('>').map(|i| start + i + 1) else {
            break;
        };
        out.push_str(&text[at..start]);
        out.push_str(if break_seconds(&lower[start..end]) >= 1.0 {
            "[long pause]"
        } else {
            "[short pause]"
        });
        at = end;
    }
    (at > 0).then(|| out + &text[at..])
}

/// The words a voiceover's `script` says, punctuation on the word before
/// it. Audio tags are not said, and a dash, an ellipsis or a full stop in
/// Chinese or Japanese ends a word without a space; so does each character
/// that is a word or close to one there.
pub fn spoken_words(script: &str) -> Vec<String> {
    let ends_word = |c: char| {
        matches!(c, '—' | '–' | '…' | '、' | '。' | '，' | '！' | '？')
            || matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF)
    };
    let mut words: Vec<String> = Vec::new();
    let text = without_tags(script);
    for part in text
        .split_whitespace()
        .flat_map(|token| token.split_inclusive(ends_word))
    {
        let said = part.chars().any(char::is_alphanumeric);
        match words.last_mut() {
            Some(word) if !said => word.push_str(part),
            _ if said => words.push(part.to_owned()),
            _ => {}
        }
    }
    words
}

/// `script` with a space for each `[audio tag]`.
fn without_tags(script: &str) -> String {
    let mut out = String::new();
    let mut rest = script;
    while let Some((said, tag)) = rest.split_once('[') {
        out.push_str(said);
        out.push(' ');
        rest = tag.split_once(']').map_or(tag, |(_, after)| after);
    }
    out + rest
}

/// Seconds in a lower-case `<break time="1.5s"/>`; 0 without a time.
fn break_seconds(tag: &str) -> f64 {
    let Some(time) = tag.split("time=").nth(1) else {
        return 0.0;
    };
    let time = time.trim_start_matches(['"', '\'']);
    let digits = time
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(time.len());
    let value: f64 = time[..digits].parse().unwrap_or(0.0);
    if time[digits..].starts_with("ms") {
        value / 1000.0
    } else {
        value
    }
}
