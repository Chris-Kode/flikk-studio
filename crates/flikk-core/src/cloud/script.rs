//! A voiceover's script: what a voiceover model can read
//! ([`VoiceoverModel`]), and the words it says.

use super::{CloudError, CloudResult, SCRIPT_CHARS};

/// What one voiceover model reads and which voices it can use, so a script
/// is checked before it costs anything and the form, the Voices dialog and
/// the agent offer only what the model takes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoiceoverModel {
    pub name: &'static str,
    /// The most characters one script has; never above [`SCRIPT_CHARS`].
    pub script_chars: usize,
    /// How delivery is written into the script.
    pub marks: DeliveryMarks,
    /// It reads [`Reading`](super::Reading)'s stability and similarity.
    pub tuned: bool,
    /// What can be done with its provider's voices beyond picking one.
    pub voices: VoiceOps,
}

/// How a model reads delivery written into a script.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryMarks {
    /// Bracketed tags: `[whispers]`, `[laughs]`, `[short pause]`.
    AudioTags,
    /// The [`INTERJECTIONS`] in parentheses, and `<#0.5#>` pauses in
    /// seconds.
    Interjections,
    /// None: only punctuation. Anything in brackets is read aloud.
    Plain,
}

/// What the provider can do with voices for a model, beyond reading with
/// one of its own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VoiceOps {
    /// Search a shared library of other people's voices and add them.
    pub library: bool,
    /// Make a voice from a description.
    pub design: bool,
    /// Clone a voice from recordings, which this model then reads with.
    pub clone: bool,
    /// Take the background noise out of recordings before cloning.
    pub denoise: bool,
}

impl VoiceOps {
    /// Nothing but reading with the model's own voices.
    pub const NONE: Self = Self {
        library: false,
        design: false,
        clone: false,
        denoise: false,
    };
}

/// The interjections a model with [`DeliveryMarks::Interjections`] reads.
pub const INTERJECTIONS: [&str; 8] = [
    "(laughs)",
    "(sighs)",
    "(coughs)",
    "(clears throat)",
    "(gasps)",
    "(sniffs)",
    "(groans)",
    "(yawns)",
];

impl Default for VoiceoverModel {
    /// What the form shows before a model is picked: the longest script,
    /// audio tags and tuning, and nothing to do with voices.
    fn default() -> Self {
        Self {
            name: "The voiceover model",
            script_chars: SCRIPT_CHARS,
            marks: DeliveryMarks::AudioTags,
            tuned: true,
            voices: VoiceOps::default(),
        }
    }
}

impl VoiceoverModel {
    /// Why `text` can't be read, with the rewrite that can when there is
    /// one, before any money is spent on it.
    pub fn check(&self, text: &str) -> CloudResult<()> {
        let refuse = |message: String, suggestion| {
            Err(CloudError::BadPrompt {
                message,
                suggestion,
            })
        };
        let chars = text.chars().count();
        let most = self.script_chars.min(SCRIPT_CHARS);
        if chars > most {
            return refuse(
                format!(
                    "{} reads at most {most} characters; this has {chars}",
                    self.name
                ),
                None,
            );
        }
        match self.without_breaks(text) {
            Some(fixed) => refuse("<break> tags aren't read; use pauses".into(), Some(fixed)),
            None => Ok(()),
        }
    }

    /// `text` with its SSML `<break/>` tags as the pauses this model reads;
    /// `None` when it has none.
    pub fn without_breaks(&self, text: &str) -> Option<String> {
        // Lower case keeps every byte where it is.
        let lower = text.to_ascii_lowercase();
        let mut out = String::new();
        let mut at = 0;
        while let Some(start) = lower[at..].find("<break").map(|i| at + i) {
            let Some(end) = lower[start..].find('>').map(|i| start + i + 1) else {
                break;
            };
            out.push_str(&text[at..start]);
            out.push_str(&self.marks.pause(break_seconds(&lower[start..end])));
            at = end;
        }
        (at > 0).then(|| out + &text[at..])
    }
}

impl DeliveryMarks {
    /// A pause of about `secs`, as these marks write it.
    pub fn pause(self, secs: f64) -> String {
        match self {
            Self::AudioTags if secs >= 1.0 => "[long pause]".into(),
            Self::AudioTags => "[short pause]".into(),
            Self::Interjections => {
                let secs = format!("{:.2}", secs.clamp(0.01, 99.99));
                format!("<#{}#>", secs.trim_end_matches('0').trim_end_matches('.'))
            }
            Self::Plain => "...".into(),
        }
    }
}

/// The words a voiceover's `script` says, punctuation on the word before
/// it. Delivery marks are not said, and a dash, an ellipsis or a full stop
/// in Chinese or Japanese ends a word without a space; so does each
/// character that is a word or close to one there.
pub fn spoken_words(script: &str) -> Vec<String> {
    let ends_word = |c: char| {
        matches!(c, '—' | '–' | '…' | '、' | '。' | '，' | '！' | '？')
            || matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF)
    };
    let mut words: Vec<String> = Vec::new();
    let text = without_marks(script);
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

/// `script` with a space for each delivery mark of any model: an
/// `[audio tag]`, a `<#0.5#>` pause or one of the [`INTERJECTIONS`].
fn without_marks(script: &str) -> String {
    let mut out = String::new();
    let mut rest = script;
    while let Some(start) = rest.find(['[', '<', '(']) {
        let (said, mark) = rest.split_at(start);
        let end = match mark.as_bytes()[0] {
            b'[' => mark.find(']').map(|i| i + 1),
            b'<' if mark.starts_with("<#") => mark[2..].find("#>").map(|i| i + 4),
            _ => INTERJECTIONS
                .iter()
                .find(|i| mark.starts_with(*i))
                .map(|i| i.len()),
        };
        out.push_str(said);
        match end {
            Some(end) => {
                out.push(' ');
                rest = &mark[end..];
            }
            // An unclosed `[` is dropped; any other bracket is said.
            None if mark.starts_with('[') => {
                out.push(' ');
                rest = &mark[1..];
            }
            None => {
                out.push_str(&mark[..1]);
                rest = &mark[1..];
            }
        }
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
