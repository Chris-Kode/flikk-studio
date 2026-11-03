//! Recipes for `/` completion and the quick-command bar: the user's list
//! from the store, or the seven built-ins (PRD "Automations") without one.

use crate::state::EditorState;
use crate::ui::kit::IconName;

const BUILTIN: [(&str, &str); 7] = [
    ("rough-cut", "Rough cut"),
    ("dead-time", "Remove dead time"),
    ("teaser", "Teaser"),
    ("captions", "Captions"),
    ("zooms", "Zoom on emphasis"),
    ("music", "Music bed"),
    ("feedback", "Apply feedback"),
];

/// Slash name and a short title.
pub fn list(state: &EditorState) -> Vec<(String, String)> {
    let stored = state.recipes();
    if stored.is_empty() {
        return BUILTIN
            .iter()
            .map(|(n, t)| (n.to_string(), t.to_string()))
            .collect();
    }
    stored
        .into_iter()
        .map(|r| {
            let title = BUILTIN
                .iter()
                .find(|(n, _)| *n == r.name)
                .map(|(_, t)| t.to_string())
                .unwrap_or(r.description);
            (r.name, title)
        })
        .collect()
}

/// The icon the quick-command bar lists a recipe with.
pub fn icon(name: &str) -> IconName {
    match name {
        "rough-cut" => IconName::Scissors,
        "dead-time" => IconName::AudioLines,
        "teaser" => IconName::Clapperboard,
        "captions" => IconName::Captions,
        "zooms" => IconName::ZoomIn,
        "music" => IconName::Music,
        "feedback" => IconName::MessageSquareText,
        _ => IconName::WandSparkles,
    }
}
