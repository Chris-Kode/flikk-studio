//! The voices a provider reads with: its own, the shared library's, and
//! the ones the user designs or clones.

use super::{CloudError, CloudResult};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A voice a provider can read with.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Voice {
    pub id: String,
    pub name: String,
    /// "premade", "cloned", "generated"…, as the provider names them.
    pub category: String,
    /// What it is like: "calm", "american", "narration".
    pub traits: Vec<String>,
    /// Where a sample of it can be heard.
    pub sample: Option<String>,
    /// What whoever made it says of it.
    pub description: String,
    /// Shared in the provider's voice library by someone else: their public
    /// id. Such a voice is added to the account before it reads.
    pub owner: Option<String>,
    /// What a character read with it costs against the usual, when it
    /// costs more (a shared voice whose owner set a rate).
    pub rate: Option<f32>,
    /// Days of notice its owner gives before withdrawing a shared voice.
    pub notice_days: Option<u32>,
    /// ISO 639-1 language it speaks best, when the provider says.
    pub language: Option<String>,
}

impl Voice {
    /// Made by the account's owner: designed or cloned, not one of the
    /// provider's own or added from the library.
    pub fn mine(&self) -> bool {
        self.owner.is_none() && matches!(self.category.as_str(), "generated" | "cloned")
    }

    /// From the provider's shared library, not on the account yet.
    pub fn shared(&self) -> bool {
        self.owner.is_some()
    }

    /// Name, traits and description in one lower-case line, to match
    /// words against.
    pub fn about(&self) -> String {
        format!(
            "{} {} {} {}",
            self.name,
            self.category,
            self.traits.join(" "),
            self.description
        )
        .to_lowercase()
    }
}

/// What to look for in a provider's shared voice library. Empty fields
/// don't filter.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VoiceQuery {
    /// Words in the name, description or labels.
    pub text: String,
    /// One of [`VOICE_GENDERS`].
    pub gender: Option<String>,
    /// One of [`VOICE_AGES`].
    pub age: Option<String>,
    /// ISO 639-1.
    pub language: Option<String>,
    /// One of [`VOICE_USES`].
    pub use_case: Option<String>,
}

impl VoiceQuery {
    /// Whether `voice` (one of the account's) fits it; the shared library
    /// filters on the provider's side.
    pub fn fits(&self, voice: &Voice) -> bool {
        let about = voice.about();
        let words = self.text.split_whitespace().map(str::to_lowercase);
        let labels = [&self.gender, &self.age, &self.use_case];
        words.into_iter().all(|w| about.contains(&w))
            && labels
                .into_iter()
                .flatten()
                .all(|l| about.contains(&l.replace('_', " ")))
            && self
                .language
                .as_ref()
                .is_none_or(|l| voice.language.as_ref().is_none_or(|v| v == l))
    }
}

/// Values the shared library's filters take, as the provider spells them.
pub const VOICE_GENDERS: [&str; 3] = ["female", "male", "neutral"];
pub const VOICE_AGES: [&str; 3] = ["young", "middle_aged", "old"];
pub const VOICE_USES: [&str; 7] = [
    "narrative_story",
    "conversational",
    "characters_animation",
    "social_media",
    "entertainment_tv",
    "advertisement",
    "informative_educational",
];
pub const VOICE_LANGUAGES: [(&str, &str); 16] = [
    ("en", "English"),
    ("es", "Spanish"),
    ("fr", "French"),
    ("de", "German"),
    ("it", "Italian"),
    ("pt", "Portuguese"),
    ("nl", "Dutch"),
    ("pl", "Polish"),
    ("sv", "Swedish"),
    ("tr", "Turkish"),
    ("ru", "Russian"),
    ("ar", "Arabic"),
    ("hi", "Hindi"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("zh", "Chinese"),
];

/// A filter value as the user reads it: "middle_aged" → "Middle aged".
pub fn voice_label(value: &str) -> String {
    let text = value.replace('_', " ");
    let mut chars = text.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

/// One page of the shared library.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VoicePage {
    pub voices: Vec<Voice>,
    /// Whether a next page has more.
    pub more: bool,
}

/// A voice to make from a description.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VoiceDesign {
    /// Who it is and how they sound: age, gender, accent, tone, pace.
    pub description: String,
    /// What the samples say; `None` lets the provider write it.
    #[serde(default)]
    pub text: Option<String>,
}

/// How long a voice description and a sample text run, in characters.
pub const DESIGN_DESCRIPTION: (usize, usize) = (20, 1000);
pub const DESIGN_TEXT: (usize, usize) = (100, 1000);

impl VoiceDesign {
    /// Why the provider would refuse it, before any credits are spent.
    pub fn check(&self) -> CloudResult<()> {
        let refuse = |message: String| {
            Err(CloudError::BadPrompt {
                message,
                suggestion: None,
            })
        };
        let (min, max) = DESIGN_DESCRIPTION;
        let n = self.description.trim().chars().count();
        if n < min || n > max {
            return refuse(format!(
                "A voice description runs {min} to {max} characters; this has {n}"
            ));
        }
        if let Some(text) = &self.text {
            let (min, max) = DESIGN_TEXT;
            let n = text.trim().chars().count();
            if n < min || n > max {
                return refuse(format!(
                    "A sample text runs {min} to {max} characters; this has {n}"
                ));
            }
        }
        Ok(())
    }
}

/// One of the voices a design offers, heard before it is kept.
#[derive(Clone, Debug, PartialEq)]
pub struct VoicePreview {
    /// The provider's id for it until it is saved.
    pub id: String,
    /// Its sample, under `paths::cache()`.
    pub audio: PathBuf,
}

/// What a design came back with.
#[derive(Clone, Debug, PartialEq)]
pub struct Designed {
    pub previews: Vec<VoicePreview>,
    /// What the samples say.
    pub text: String,
}

/// A voice to clone from recordings of it.
#[derive(Clone, Debug, PartialEq)]
pub struct VoiceClone {
    pub name: String,
    pub description: String,
    /// Clean speech, one voice; a minute or more together.
    pub files: Vec<PathBuf>,
    /// Have the provider take out background noise first.
    pub denoise: bool,
}

/// How a voice in the user's library came to be.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VoiceOrigin {
    /// Designed from a description, which can design it again.
    Designed {
        description: String,
        #[serde(default)]
        text: Option<String>,
    },
    /// Cloned from recordings, named here.
    Cloned { sources: Vec<String> },
    /// Added from the provider's shared library.
    Shared {
        owner: String,
        /// Its name there.
        name: String,
    },
    /// On the account already (one of the provider's, or made elsewhere).
    Account,
}
