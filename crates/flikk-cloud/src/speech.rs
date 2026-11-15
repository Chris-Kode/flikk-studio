//! What the voiceover clients of hosts share (fal, Pika: they run other
//! vendors' speech models behind one key). Each hosted model is a
//! [`HostedSpeech`] saying which of its fields carry the script, the voice
//! and its tuning; [`input`] writes a request in those fields, and
//! [`presets`] lists the voices it reads with, which no host lists itself.
//! The file comes back as music does (`crate::audio::{made, fetch}`).

use crate::audio::Fixed;
use flikk_core::cloud::{
    CloudError, CloudResult, GenerateRequest, Voice, VoiceoverModel, voice_label,
};
use serde_json::{Map, Value, json};

/// One voiceover model a host runs, and how it takes a request.
pub struct HostedSpeech {
    /// The catalog's id, which is also the endpoint called.
    pub id: &'static str,
    pub spec: VoiceoverModel,
    /// The field the script goes in ("text", "prompt", "script").
    pub text: &'static str,
    /// Where the voice id goes: a field, or `outer.inner` for one inside
    /// an object.
    pub voice: &'static str,
    /// Where stability and similarity go, as `voice` says; `Some` when
    /// `spec.tuned`.
    pub tuning: Option<(&'static str, &'static str)>,
    /// Sent with every request, as `voice` says: the file format, a
    /// lossless one where the model makes a real file of it.
    pub fixed: &'static [(&'static str, Fixed)],
    /// The voices it reads with, the default first.
    pub voices: &'static [&'static str],
    /// Whether an id outside `voices` is one of the account's clones it
    /// reads; `None` when it reads none.
    pub clone_id: Option<fn(&str) -> bool>,
}

/// The voices MiniMax Speech reads with on every host, the default first.
pub const MINIMAX_VOICES: &[&str] = &[
    "Wise_Woman",
    "Friendly_Person",
    "Inspirational_girl",
    "Deep_Voice_Man",
    "Calm_Woman",
    "Casual_Guy",
    "Lively_Girl",
    "Patient_Man",
    "Young_Knight",
    "Determined_Man",
    "Lovely_Girl",
    "Decent_Boy",
    "Imposing_Manner",
    "Elegant_Man",
    "Abbess",
    "Sweet_Girl_2",
    "Exuberant_Girl",
];

impl HostedSpeech {
    /// Whether it reads with `voice`.
    fn reads(&self, voice: &str) -> bool {
        self.voices.contains(&voice) || self.clone_id.is_some_and(|clone| clone(voice))
    }
}

/// The input `model` takes for `request`, a voiceover. A voice it doesn't
/// read is refused before anything is spent. Hosted models take no speech
/// around the script, so a word fix reads without it.
pub fn input(model: &HostedSpeech, request: &GenerateRequest) -> CloudResult<Value> {
    let GenerateRequest::Voiceover { text, reading, .. } = request else {
        return Err(CloudError::Failed("not a voiceover".into()));
    };
    if !model.reads(&reading.voice) {
        return Err(CloudError::BadPrompt {
            message: format!(
                "{} has no voice “{}”; pick one of its own",
                model.spec.name, reading.voice_name
            ),
            suggestion: None,
        });
    }
    let mut input = Map::new();
    for (field, value) in model.fixed {
        put(&mut input, field, value.value());
    }
    put(&mut input, model.text, json!(text));
    put(&mut input, model.voice, json!(reading.voice));
    if let Some((stability, similarity)) = model.tuning {
        put(
            &mut input,
            stability,
            json!(reading.stability.clamp(0.0, 1.0)),
        );
        put(
            &mut input,
            similarity,
            json!(reading.similarity.clamp(0.0, 1.0)),
        );
    }
    Ok(Value::Object(input))
}

/// The voices `model` reads with, named from their ids:
/// "calm_documentary_narrator" is "Calm documentary narrator".
pub fn presets(model: &HostedSpeech) -> Vec<Voice> {
    model
        .voices
        .iter()
        .map(|id| Voice {
            id: (*id).to_owned(),
            name: voice_label(id),
            category: "premade".into(),
            ..Voice::default()
        })
        .collect()
}

/// Set `path` (a field, or `outer.inner`) in `input` to `value`.
fn put(input: &mut Map<String, Value>, path: &str, value: Value) {
    match path.split_once('.') {
        Some((outer, inner)) => {
            let entry = input
                .entry(outer.to_owned())
                .or_insert_with(|| Value::Object(Map::new()));
            if let Value::Object(fields) = entry {
                put(fields, inner, value);
            }
        }
        None => {
            input.insert(path.to_owned(), value);
        }
    }
}
