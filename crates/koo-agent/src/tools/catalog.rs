//! One JSON schema per tool (PRD "Agent tool API"). Times are timeline
//! seconds; strings like "1:02.500" are accepted too.

use koo_core::command::ToolCategory;
use serde_json::{Value, json};

pub struct ToolSpec {
    pub name: &'static str,
    /// `None` for `ask_user`, which is always available.
    pub category: Option<ToolCategory>,
    pub description: &'static str,
    pub schema: Value,
}

impl ToolSpec {
    /// MCP tool definition.
    pub fn to_mcp(&self) -> Value {
        let read_only = self.category.is_none_or(|c| c.is_read());
        json!({
            "name": self.name,
            "description": self.description,
            "inputSchema": self.schema,
            "annotations": { "readOnlyHint": read_only },
        })
    }
}

fn time(desc: &str) -> Value {
    json!({ "type": "number", "description": desc })
}

fn range(desc: &str) -> Value {
    json!({
        "type": "object",
        "description": desc,
        "properties": {
            "start": { "type": "number", "description": "Seconds" },
            "end": { "type": "number", "description": "Seconds" }
        },
        "required": ["start", "end"]
    })
}

fn string(desc: &str) -> Value {
    json!({ "type": "string", "description": desc })
}

fn number(desc: &str) -> Value {
    json!({ "type": "number", "description": desc })
}

fn strings(desc: &str) -> Value {
    json!({ "type": "array", "items": { "type": "string" }, "description": desc })
}

fn one_of(values: &[&str], desc: &str) -> Value {
    json!({ "type": "string", "enum": values, "description": desc })
}

fn reason() -> Value {
    string("One line shown to the user with the change; cite the style rule that drove it, if any")
}

fn object(props: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": props, "required": required })
}

fn spec(
    name: &'static str,
    category: Option<ToolCategory>,
    description: &'static str,
    schema: Value,
) -> ToolSpec {
    ToolSpec {
        name,
        category,
        description,
        schema,
    }
}

pub fn catalog() -> Vec<ToolSpec> {
    use ToolCategory::*;
    let eases = &["linear", "ease_in_out", "ease_in", "ease_out", "hold"];
    let edges = &["start", "end"];
    vec![
        // ---- read project
        spec(
            "get_timeline",
            Some(ReadProject),
            "Tracks, clips and markers overlapping a range (default: whole timeline), including your pending proposals.",
            object(json!({ "range": range("Timeline range") }), &[]),
        ),
        spec(
            "get_selection",
            Some(ReadProject),
            "What the user has selected: clips, transcript words, in/out and playhead.",
            object(json!({}), &[]),
        ),
        spec(
            "list_media",
            Some(ReadProject),
            "Media pool items, optionally in one bin.",
            object(json!({ "bin": string("Bin name") }), &[]),
        ),
        spec(
            "get_clip",
            Some(ReadProject),
            "Everything about one clip: source, transform, keyframes, grade, audio.",
            object(json!({ "clip": string("Clip id") }), &["clip"]),
        ),
        spec(
            "get_markers",
            Some(ReadProject),
            "Markers and protected ranges overlapping a range (default: whole timeline).",
            object(json!({ "range": range("Timeline range") }), &[]),
        ),
        // ---- read analysis
        spec(
            "search_transcript",
            Some(ReadAnalysis),
            "Find where words are said on the timeline. Case and punctuation are ignored.",
            object(
                json!({ "text": string("Words to find"), "range": range("Limit to this timeline range") }),
                &["text"],
            ),
        ),
        spec(
            "get_transcript",
            Some(ReadAnalysis),
            "Word-level transcript of a timeline range, grouped into lines by speaker and pauses. Each word is `word@seconds`.",
            object(json!({ "range": range("Timeline range") }), &["range"]),
        ),
        spec(
            "get_speakers",
            Some(ReadAnalysis),
            "Speakers in a range with talk time and first appearance.",
            object(
                json!({ "range": range("Timeline range (default: whole timeline)") }),
                &[],
            ),
        ),
        spec(
            "find_silences",
            Some(ReadAnalysis),
            "Gaps without speech of at least `min_s` seconds, in timeline time.",
            object(
                json!({ "min_s": number("Shortest gap, seconds (default 0.5)"), "range": range("Timeline range") }),
                &[],
            ),
        ),
        spec(
            "find_shot_changes",
            Some(ReadAnalysis),
            "Timeline times where the picture cuts to a new shot.",
            object(json!({ "range": range("Timeline range") }), &[]),
        ),
        // ---- view
        spec(
            "view_range",
            Some(View),
            "See a range: its transcript plus a collage of up to 30 frames spread evenly over it, each stamped with its timeline time.",
            object(json!({ "range": range("Timeline range") }), &["range"]),
        ),
        spec(
            "view_detail",
            Some(View),
            "A denser collage of a short span (at most 20 s), for checking a cut or a moment.",
            object(
                json!({ "range": range("Timeline range, at most 20 s") }),
                &["range"],
            ),
        ),
        spec(
            "view_frame",
            Some(View),
            "One full frame of the program at a timeline time.",
            object(json!({ "t": time("Timeline seconds") }), &["t"]),
        ),
        spec(
            "get_scopes",
            Some(View),
            "Waveform, vectorscope and histogram readings of a clip.",
            object(json!({ "clip": string("Clip id") }), &["clip"]),
        ),
        spec(
            "measure_loudness",
            Some(View),
            "Integrated loudness (LUFS), true peak and loudness range of the audio in a range, per clip.",
            object(
                json!({ "range": range("Timeline range (default: whole timeline)") }),
                &[],
            ),
        ),
        // ---- edit timeline
        spec(
            "blade",
            Some(EditTimeline),
            "Propose cutting clips at a time.",
            object(
                json!({ "at": time("Timeline seconds"), "tracks": strings("Track names or ids (default: all unlocked)"), "reason": reason() }),
                &["at"],
            ),
        ),
        spec(
            "ripple_delete",
            Some(EditTimeline),
            "Propose removing a range and closing the gap.",
            object(
                json!({ "range": range("Timeline range to remove"), "tracks": strings("Track names or ids (default: all unlocked)"), "reason": reason() }),
                &["range"],
            ),
        ),
        spec(
            "insert_clip",
            Some(EditTimeline),
            "Propose placing a media item (or part of it) on a track.",
            object(
                json!({
                    "media": string("Media id"),
                    "source": range("Source range in the media's own time (default: all of it)"),
                    "track": string("Track name or id (default: first video or audio track)"),
                    "at": time("Timeline seconds"),
                    "mode": one_of(&["insert", "overwrite"], "insert pushes later clips right (default)"),
                    "reason": reason()
                }),
                &["media", "at"],
            ),
        ),
        spec(
            "move_clip",
            Some(EditTimeline),
            "Propose moving a clip to a new start time and optionally another track.",
            object(
                json!({ "clip": string("Clip id"), "at": time("New start, timeline seconds"), "track": string("Track name or id"), "reason": reason() }),
                &["clip", "at"],
            ),
        ),
        spec(
            "trim",
            Some(EditTimeline),
            "Propose moving a clip's start or end edge. Positive `delta` moves the edge later.",
            object(
                json!({
                    "clip": string("Clip id"),
                    "edge": one_of(edges, "Which edge"),
                    "delta": number("Seconds; positive = later"),
                    "ripple": { "type": "boolean", "description": "Shift later clips to keep them attached (default false)" },
                    "reason": reason()
                }),
                &["clip", "edge", "delta"],
            ),
        ),
        spec(
            "add_marker",
            Some(EditTimeline),
            "Propose a marker at a time or over a range. `protected` ranges must not be cut.",
            object(
                json!({
                    "at": time("Point marker, timeline seconds"),
                    "range": range("Range marker"),
                    "kind": one_of(&["note", "protected"], "Default note"),
                    "note": string("Marker text"),
                    "reason": reason()
                }),
                &["note"],
            ),
        ),
        // ---- transform
        spec(
            "add_zoom",
            Some(Transform),
            "Propose a punch-in: ease in to `scale` around `target`, hold over the range, ease out.",
            object(
                json!({
                    "clip": string("Clip id"),
                    "range": range("Timeline range of the zoom"),
                    "target": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2, "description": "[x, y] offset from center, -1..1 (default [0, 0])" },
                    "scale": number("Default from the style profile, usually 1.2–2"),
                    "ease": one_of(eases, "Default ease_in_out"),
                    "reason": reason()
                }),
                &["clip", "range"],
            ),
        ),
        spec(
            "set_keyframe",
            Some(Transform),
            "Propose a keyframe on a clip property at a timeline time.",
            object(
                json!({
                    "clip": string("Clip id"),
                    "property": one_of(&["position_x", "position_y", "scale", "rotation", "opacity"], "Property"),
                    "at": time("Timeline seconds, inside the clip"),
                    "value": number("Value"),
                    "ease": one_of(eases, "Easing into the next keyframe (default ease_in_out)"),
                    "reason": reason()
                }),
                &["clip", "property", "at", "value"],
            ),
        ),
        // ---- captions and titles
        spec(
            "generate_captions",
            Some(Captions),
            "Propose captions from the transcript for a range (default: whole timeline), split on pauses and sentence ends.",
            object(
                json!({
                    "range": range("Timeline range"),
                    "track": string("Caption track name or id (default: first caption track)"),
                    "max_chars": number("Longest caption in characters (default from the caption style)"),
                    "reason": reason()
                }),
                &[],
            ),
        ),
        spec(
            "edit_caption",
            Some(Captions),
            "Propose new text and/or timing for a caption clip.",
            object(
                json!({ "clip": string("Caption clip id"), "text": string("New text"), "range": range("New timeline range"), "reason": reason() }),
                &["clip"],
            ),
        ),
        spec(
            "apply_title",
            Some(Captions),
            "Propose a title over a range.",
            object(
                json!({
                    "template": string("Title template name, e.g. lower_third"),
                    "text": string("Title text"),
                    "range": range("Timeline range"),
                    "track": string("Title track name or id (default: first title track, created if missing)"),
                    "reason": reason()
                }),
                &["template", "text", "range"],
            ),
        ),
        // ---- color
        spec(
            "set_primary",
            Some(Color),
            "Propose primary correction on clips. Only the values given change.",
            object(
                json!({
                    "clips": strings("Clip ids"),
                    "exposure": number("Stops, 0 = neutral"),
                    "contrast": number("1 = neutral"),
                    "saturation": number("1 = neutral"),
                    "gamma": number("1 = neutral"),
                    "temperature": number("0 = neutral, negative cooler"),
                    "tint": number("0 = neutral, negative greener"),
                    "lift": { "type": "array", "items": { "type": "number" }, "description": "[r, g, b], 0 = neutral" },
                    "gamma_rgb": { "type": "array", "items": { "type": "number" }, "description": "[r, g, b], 1 = neutral" },
                    "gain": { "type": "array", "items": { "type": "number" }, "description": "[r, g, b], 1 = neutral" },
                    "reason": reason()
                }),
                &["clips"],
            ),
        ),
        spec(
            "apply_lut",
            Some(Color),
            "Propose a LUT file on clips; omit `lut` to remove it.",
            object(
                json!({ "clips": strings("Clip ids"), "lut": string("Path of a .cube file"), "reason": reason() }),
                &["clips"],
            ),
        ),
        spec(
            "copy_grade",
            Some(Color),
            "Propose copying one clip's grade to others.",
            object(
                json!({ "from": string("Clip id"), "to": strings("Clip ids"), "reason": reason() }),
                &["from", "to"],
            ),
        ),
        spec(
            "match_grade",
            Some(Color),
            "Propose grades that match clips to a reference clip.",
            object(
                json!({ "reference": string("Clip id"), "clips": strings("Clip ids"), "reason": reason() }),
                &["reference", "clips"],
            ),
        ),
        // ---- audio
        spec(
            "set_gain",
            Some(Audio),
            "Propose a clip gain in dB.",
            object(
                json!({ "clip": string("Clip id"), "db": number("Gain, dB"), "reason": reason() }),
                &["clip", "db"],
            ),
        ),
        spec(
            "add_fade",
            Some(Audio),
            "Propose an audio fade at a clip's start or end.",
            object(
                json!({ "clip": string("Clip id"), "edge": one_of(edges, "Which edge"), "duration": number("Seconds"), "reason": reason() }),
                &["clip", "edge", "duration"],
            ),
        ),
        spec(
            "set_ducking",
            Some(Audio),
            "Propose ducking a music track under dialogue; omit `music_track` to turn ducking off.",
            object(
                json!({
                    "music_track": string("Track name or id"),
                    "reduction_db": number("Default from the style profile"),
                    "attack": number("Seconds (default 0.2)"),
                    "release": number("Seconds (default 0.5)"),
                    "reason": reason()
                }),
                &[],
            ),
        ),
        spec(
            "set_playlist",
            Some(Audio),
            "Propose songs laid back to back on a track from a time, replacing what was there.",
            object(
                json!({ "track": string("Audio track name or id"), "at": time("Timeline seconds"), "songs": strings("Media ids in order"), "reason": reason() }),
                &["track", "at", "songs"],
            ),
        ),
        spec(
            "normalize",
            Some(Audio),
            "Measure clips and propose gains that bring each to a loudness target.",
            object(
                json!({
                    "clips": strings("Clip ids (default: audio clips in `range`)"),
                    "range": range("Timeline range (default: whole timeline)"),
                    "target_lufs": number("Default from the style profile, usually -14"),
                    "reason": reason()
                }),
                &[],
            ),
        ),
        spec(
            "place_sfx",
            Some(Audio),
            "Propose a sound effect from the media pool at a time.",
            object(
                json!({ "media": string("Media id"), "at": time("Timeline seconds"), "track": string("Audio track (default: last audio track)"), "reason": reason() }),
                &["media", "at"],
            ),
        ),
        // ---- overlays
        spec(
            "add_overlay",
            Some(Overlays),
            "Propose an image or asset overlay over a range.",
            object(
                json!({
                    "asset": string("Asset path"),
                    "range": range("Timeline range"),
                    "speaker": string("Speaker id to tie it to"),
                    "track": string("Overlay track (default: first overlay track, created if missing)"),
                    "reason": reason()
                }),
                &["asset", "range"],
            ),
        ),
        // ---- project and memory (approval)
        spec(
            "propose_rule",
            Some(ProjectMemory),
            "Ask the user to approve a style rule, learned from their feedback or edits.",
            object(
                json!({
                    "text": string("The rule, as an instruction"),
                    "scope": one_of(&["global", "project", "next_video"], "Default global"),
                    "evidence": strings("Why: quotes from feedback, change set ids"),
                    "supersedes": string("Rule id this replaces")
                }),
                &["text"],
            ),
        ),
        spec(
            "save_recipe",
            Some(ProjectMemory),
            "Ask the user to approve a reusable recipe (a slash command).",
            object(
                json!({
                    "name": string("Slash name, lowercase-with-dashes"),
                    "description": string("One line"),
                    "prompt": string("Instructions; {param} placeholders allowed"),
                    "params": { "type": "array", "items": { "type": "object", "properties": { "name": { "type": "string" }, "description": { "type": "string" }, "default": { "type": "string" } }, "required": ["name"] } }
                }),
                &["name", "prompt"],
            ),
        ),
        spec(
            "add_source_fact",
            Some(ProjectMemory),
            "Ask the user to confirm a fact about a source (who a speaker is, on-screen text, a literal quote).",
            object(
                json!({
                    "media": string("Media id"),
                    "kind": one_of(&["speaker_reference", "original_audio_track", "literal_quote", "on_screen"], "Kind of fact"),
                    "text": string("The fact"),
                    "source_range": range("Range in the media's own time, if the fact is about a moment")
                }),
                &["media", "kind", "text"],
            ),
        ),
        // ---- deliver (approval)
        spec(
            "queue_render",
            Some(Deliver),
            "Ask the user to approve a render.",
            object(
                json!({ "preset": string("Preset name, e.g. youtube_1080p"), "range": range("Timeline range (default: whole timeline)") }),
                &["preset"],
            ),
        ),
        spec(
            "export_otio",
            Some(Deliver),
            "Ask the user to approve exporting the timeline as OpenTimelineIO.",
            object(json!({}), &[]),
        ),
        spec(
            "export_frame",
            Some(Deliver),
            "Ask the user to approve exporting a still frame.",
            object(json!({ "t": time("Timeline seconds") }), &["t"]),
        ),
        // ---- conversation
        spec(
            "ask_user",
            None,
            "Ask the user a clarifying question with clickable choices, then end your turn; the answer arrives as the next message.",
            object(
                json!({ "question": string("The question"), "choices": strings("Two to four short answers") }),
                &["question", "choices"],
            ),
        ),
    ]
}
