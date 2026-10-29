//! One JSON schema per tool (PRD "Agent tool API"). Times are timeline
//! seconds; strings like "1:02.500", timecode "00:01:02:30" and frames
//! "f:3750" are accepted too.

use super::layout;
use koo_core::audiofx::AudioFxKind;
use koo_core::command::ToolCategory;
use koo_core::model::{BlendMode, ClipColor, Property};
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

/// Seconds, "M:SS.mmm", timecode "HH:MM:SS:FF" or frames "f:1234".
fn time(desc: &str) -> Value {
    json!({ "type": ["number", "string"], "description": desc })
}

fn range(desc: &str) -> Value {
    json!({
        "type": "object",
        "description": desc,
        "properties": {
            "start": time("Seconds or a time string"),
            "end": time("Seconds or a time string")
        },
        "required": ["start", "end"]
    })
}

fn boolean(desc: &str) -> Value {
    json!({ "type": "boolean", "description": desc })
}

fn numbers(n: usize, desc: &str) -> Value {
    json!({ "type": "array", "items": { "type": "number" }, "minItems": n, "maxItems": n, "description": desc })
}

fn params(desc: &str) -> Value {
    json!({ "type": "object", "description": desc, "additionalProperties": true })
}

fn curve(desc: &str) -> Value {
    json!({
        "type": "array",
        "items": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 },
        "description": desc
    })
}

/// Grade fields shared by `set_primary` and `set_timeline_grade`.
fn grade_props(mut props: Value) -> Value {
    let rgb = |d: &str| json!({ "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3, "description": d });
    let extra = json!({
        "exposure": number("Stops, 0 = neutral"),
        "contrast": number("1 = neutral"),
        "saturation": number("1 = neutral"),
        "gamma": number("1 = neutral"),
        "temperature": number("0 = neutral, negative cooler"),
        "tint": number("0 = neutral, negative greener"),
        "lift": rgb("[r, g, b], 0 = neutral"),
        "gamma_rgb": rgb("[r, g, b], 1 = neutral"),
        "gain": rgb("[r, g, b], 1 = neutral"),
        "offset": rgb("[r, g, b] added to every channel, 0 = neutral"),
        "shadows": number("-1..1, 0 = neutral"),
        "highlights": number("-1..1, 0 = neutral"),
        "hue": number("Hue rotation in degrees, 0 = neutral"),
        "master": curve("Master curve points [[in, out], …] in 0..1; [] resets"),
        "red": curve("Red curve points"),
        "green": curve("Green curve points"),
        "blue": curve("Blue curve points"),
        "reason": reason()
    });
    if let (Some(p), Some(e)) = (props.as_object_mut(), extra.as_object()) {
        p.extend(e.clone());
    }
    props
}

fn audio_effect_item() -> Value {
    let kinds: Vec<_> = AudioFxKind::ALL.iter().map(|k| k.name()).collect();
    json!({
        "type": "object",
        "properties": {
            "kind": { "type": "string", "enum": kinds },
            "params": params("Parameter name → number (list_fx kind=audio)"),
            "enabled": { "type": "boolean" },
            "id": string("Keep an existing effect's id")
        },
        "required": ["kind"]
    })
}

/// Where a cut may move to, on music or onto the start of a sound.
fn align() -> Value {
    json!({
        "type": "string",
        "enum": ["beat", "downbeat", "onset"],
        "description": "Move the cut onto the nearest beat (within half a beat), downbeat (within half a bar) or onset, the start of a sound (within 150 ms); one clear of words wins over a nearer one on a word"
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
    let properties: Vec<&str> = Property::ALL.iter().map(|p| p.name()).collect();
    let blends: Vec<String> = BlendMode::ALL
        .iter()
        .map(|b| super::edit::blend_name(*b))
        .collect();
    let blends: Vec<&str> = blends.iter().map(String::as_str).collect();
    let mut colors: Vec<String> = ClipColor::ALL
        .iter()
        .map(|c| c.label().to_lowercase())
        .collect();
    colors.push("none".into());
    let colors: Vec<&str> = colors.iter().map(String::as_str).collect();
    let audio_kinds: Vec<&str> = AudioFxKind::ALL.iter().map(|k| k.name()).collect();
    let fx_kinds = &["effect", "transition", "generator"];
    let aligns = &["center", "start_on_cut", "end_on_cut"];
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
            "Everything about one clip: source, name, color, speed, enabled, transform (position, scale, stretch, rotation, skew, anchor, opacity, crop, flip, blend), keyframes, grade and curves, effects with their values and keyframes, transitions (and where they play), generator values, audio and audio effects.",
            object(json!({ "clip": string("Clip id") }), &["clip"]),
        ),
        spec(
            "get_markers",
            Some(ReadProject),
            "Markers and protected ranges overlapping a range (default: whole timeline).",
            object(json!({ "range": range("Timeline range") }), &[]),
        ),
        spec(
            "list_fx",
            Some(ReadProject),
            "Effects, transitions and generators you can use: built-ins and this project's own programs (GLSL or HTML), plus audio effect kinds. Filter by kind, language, category or text.",
            object(
                json!({
                    "kind": one_of(&["effect", "transition", "generator", "audio"], "Only this kind"),
                    "language": one_of(&["glsl", "html"], "Only this language"),
                    "category": string("Library group, e.g. Blur"),
                    "text": string("Words in the name or description"),
                    "params": boolean("Include full parameter specs (types, ranges, defaults); default names only")
                }),
                &[],
            ),
        ),
        spec(
            "get_fx",
            Some(ReadProject),
            "One FX program: its code, parameter specs and the clips that use it. Read a built-in before writing a similar one.",
            object(
                json!({ "fx": string("Built-in key, project id or `@fx` token") }),
                &["fx"],
            ),
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
        spec(
            "get_audio",
            Some(ReadAnalysis),
            "The sound script of a timeline range, in time order: transcript lines, [sounds] (\"?\" when unsure), ♪ music with its genre, mood, tempo and first downbeat, where the music gets louder or softer, and (silence).",
            object(json!({ "range": range("Timeline range") }), &["range"]),
        ),
        spec(
            "get_music",
            Some(ReadAnalysis),
            "Music in a range: tags, tempo, bar length, beats and downbeats in timeline seconds, and where it gets louder or softer (a drop, a breakdown).",
            object(
                json!({ "range": range("Timeline range (default: whole timeline)") }),
                &[],
            ),
        ),
        spec(
            "search_sounds",
            Some(ReadAnalysis),
            "Find where the audio sounds like a description (\"glass breaking\", \"crowd cheering\") or like a range of the timeline. Returns ranked ranges, good to about 2 s, with similarity scores and what the sound script hears there.",
            object(
                json!({
                    "text": string("What it sounds like, in a few words"),
                    "like": range("Or: a timeline range that sounds like what to find"),
                    "range": range("Where to look (default: whole timeline)"),
                    "limit": number("Most matches (default 8)")
                }),
                &[],
            ),
        ),
        // ---- view
        spec(
            "view_range",
            Some(View),
            "See a range: its sound script plus a collage of up to 30 frames spread evenly over it, each stamped with its timeline time.",
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
            "One full frame of the program at a timeline time, captions and titles included. With `annotate`, every visible item gets a numbered box, listed with its clip.",
            object(
                json!({
                    "t": time("Timeline seconds"),
                    "annotate": boolean("Box and number each visible item")
                }),
                &["t"],
            ),
        ),
        spec(
            "get_layout",
            Some(View),
            "Where everything visible at a time is on screen, topmost first: each item's box around what it draws (pixels and fractions of the frame), its transform, whether it sits inside the title-safe area, and which items overlap. Use it before placing things so they don't cover each other or faces.",
            object(
                json!({ "t": time("Timeline seconds (default: the playhead)") }),
                &[],
            ),
        ),
        spec(
            "view_audio",
            Some(View),
            "Look at up to 30 s of audio: a spectrogram per track under a time ruler, words, sounds and beats marked below, and the sound script. For an exact edge or the shape of a sound.",
            object(
                json!({ "range": range("Timeline range, at most 30 s") }),
                &["range"],
            ),
        ),
        spec(
            "get_scopes",
            Some(View),
            "Scope readings of one frame in numbers: luma and RGB levels (min, 5th percentile, mean, median, 95th, max), saturation, crushed and clipped fractions, color cast and a luma histogram. The clip's picture with its grades and effects, before its transform.",
            object(
                json!({
                    "clip": string("Clip id (default: the topmost video clip at `t`)"),
                    "t": time("Timeline time (default: the clip's middle, or the playhead)")
                }),
                &[],
            ),
        ),
        spec(
            "preview_fx",
            Some(View),
            "Look at an FX program over time: one collage of 8–12 timestamped frames. Generators render alone; effects run on a frame of `clip` (or a test card); transitions blend `from` → `to` clips (or two test cards) at several progress values. Use it after create_fx or update_fx, and before applying.",
            object(
                json!({
                    "fx": string("Built-in key, project id or `@fx` token"),
                    "clip": string("Effect: clip whose frame to process (its own instance's values are used). Generator: a generator clip to take values from. Transition: the outgoing clip"),
                    "from": string("Transition: outgoing clip (its last frame)"),
                    "to": string("Transition: incoming clip (its first frame)"),
                    "at": time("Effect: timeline time of the input frame (default: the clip's middle)"),
                    "params": params("Parameter overrides, name → value"),
                    "duration": time("Program clock length (default: the clip's or transition's, else 3 s / 1 s)"),
                    "frames": number("Frames in the collage, 2–16 (default 9)")
                }),
                &["fx"],
            ),
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
            "Propose cutting clips at a time. With `align` the cut moves onto the nearest beat, downbeat or onset first. A time that falls on a spoken word moves to the nearest pause; the result says so.",
            object(
                json!({ "at": time("Timeline seconds"), "align": align(), "tracks": strings("Track names or ids (default: all unlocked)"), "snap": boolean("Move off spoken words (default true)"), "reason": reason() }),
                &["at"],
            ),
        ),
        spec(
            "ripple_delete",
            Some(EditTimeline),
            "Propose removing a range and closing the gap, with a short audio fade at the join. With `align` both edges move onto the nearest beat, downbeat or onset first. An edge that falls on a spoken word moves to the nearest pause; the result says so. To remove words, prefer `cut_words`.",
            object(
                json!({ "range": range("Timeline range to remove"), "align": align(), "tracks": strings("Track names or ids (default: all unlocked)"), "snap": boolean("Move edges off spoken words (default true)"), "reason": reason() }),
                &["range"],
            ),
        ),
        spec(
            "cut_words",
            Some(EditTimeline),
            "Propose removing spoken words. Give each run by the `@` times of its first and last word from `get_transcript`. The cuts go in the pauses around the run, clear of the words kept, with a short audio fade; the pause before the run stays, the one after it goes. The result says where a cut has no pause to sit in.",
            object(
                json!({
                    "runs": {
                        "type": "array",
                        "description": "Runs of consecutive words to remove",
                        "items": {
                            "type": "object",
                            "properties": {
                                "from": time("`@` time of the first word"),
                                "to": time("`@` time of the last word (default: `from`)"),
                                "text": string("The words, checked against the transcript")
                            },
                            "required": ["from"]
                        }
                    },
                    "reason": reason()
                }),
                &["runs"],
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
                    "delta": time("Seconds or a time string; positive = later"),
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
        spec(
            "set_speed",
            Some(EditTimeline),
            "Propose retiming a media clip: 2 plays twice as fast (the clip gets shorter), 0.5 half speed.",
            object(
                json!({
                    "clip": string("Clip id"),
                    "speed": number("1 = normal, 0.01–100"),
                    "ripple": boolean("Move later clips to follow the new length (default false: stop at the next clip)"),
                    "reason": reason()
                }),
                &["clip", "speed"],
            ),
        ),
        spec(
            "set_clips_enabled",
            Some(EditTimeline),
            "Propose disabling (or enabling) clips: disabled clips stay on the timeline but are not seen or heard.",
            object(
                json!({ "clips": strings("Clip ids"), "enabled": boolean("false disables"), "reason": reason() }),
                &["clips", "enabled"],
            ),
        ),
        spec(
            "rename_clip",
            Some(EditTimeline),
            "Propose a label for a clip on the timeline; omit `name` to restore the source name.",
            object(
                json!({ "clip": string("Clip id"), "name": string("New label"), "reason": reason() }),
                &["clip"],
            ),
        ),
        spec(
            "set_clip_color",
            Some(EditTimeline),
            "Propose a color label for clips.",
            object(
                json!({ "clips": strings("Clip ids"), "color": one_of(&colors, "Color, or none"), "reason": reason() }),
                &["clips", "color"],
            ),
        ),
        spec(
            "duplicate_clips",
            Some(EditTimeline),
            "Propose copies of clips (with their effects, keyframes and grade) at a time: the earliest lands at `at`, the others keep their offsets.",
            object(
                json!({
                    "clips": strings("Clip ids"),
                    "at": time("Where the earliest copy starts (default: right after the last original)"),
                    "track": string("Put every copy on this track (default: each on its own track)"),
                    "mode": one_of(&["overwrite", "insert"], "overwrite replaces what is underneath (default); insert pushes later clips right"),
                    "reason": reason()
                }),
                &["clips"],
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
                    "property": one_of(&properties, "Property; crop_* are fractions 0..1, volume is clip gain in dB"),
                    "at": time("Timeline seconds, inside the clip"),
                    "value": number("Value"),
                    "ease": one_of(eases, "Easing into the next keyframe (default ease_in_out)"),
                    "reason": reason()
                }),
                &["clip", "property", "at", "value"],
            ),
        ),
        spec(
            "remove_keyframe",
            Some(Transform),
            "Propose removing a clip property keyframe at a timeline time.",
            object(
                json!({
                    "clip": string("Clip id"),
                    "property": one_of(&properties, "Property"),
                    "at": time("Timeline time of the keyframe"),
                    "reason": reason()
                }),
                &["clip", "property", "at"],
            ),
        ),
        spec(
            "set_transform",
            Some(Transform),
            "Propose clips' transform: pictures, overlays, generators, captions and titles alike. Only the values given change. Without `at` it sets the static transform (keyframed properties keep overriding it); with `at`, properties that have keyframes get one there and the rest change statically. Captions and titles ignore crop, flip and blend.",
            object(
                json!({
                    "clip": string("Clip id"),
                    "clips": strings("More clip ids, changed alike"),
                    "position": numbers(2, "[x, y] offset from where it rests (centred; captions where their style puts them), as a fraction of the frame: [0.25, 0] moves it a quarter of the frame width right"),
                    "scale": number("1 = fitted (captions: their style's size)"),
                    "stretch": numbers(2, "[x, y] per-axis scale on top of `scale`, 1 = none"),
                    "rotation": number("Degrees clockwise, about the anchor"),
                    "skew": numbers(2, "[x, y] slant in degrees, ±80: x leans vertical edges right going down, y leans horizontal edges down going right"),
                    "anchor": numbers(2, "[x, y] the point it scales, skews and rotates about, as a fraction of its size from its centre: [-0.5, -0.5] = top-left corner. Moving it alone does not move the picture"),
                    "opacity": number("0..1"),
                    "crop": numbers(4, "[left, top, right, bottom], fractions 0..1 cut away"),
                    "flip": { "type": "array", "items": { "type": "boolean" }, "minItems": 2, "maxItems": 2, "description": "[horizontal, vertical]" },
                    "blend": one_of(&blends, "Composite mode over the tracks below"),
                    "relative": boolean("Position, rotation, skew and anchor add to the current values; scale, stretch and opacity multiply them"),
                    "at": time("Timeline time to set it at, keyframe-aware (see above)"),
                    "reason": reason()
                }),
                &[],
            ),
        ),
        spec(
            "place",
            Some(Transform),
            "Propose putting a clip somewhere by meaning: align what it draws (its opaque part: a logo's pixels, a lower third inside its frame, a caption's box) to a corner, edge or the centre, a margin in from the frame edges, and size it. Keyframe-aware at `at`. Check the result with get_layout or view_frame.",
            object(
                json!({
                    "clip": string("Clip id"),
                    "align": one_of(&layout::ALIGNS, "Where it goes (default center)"),
                    "margin": number("Distance from the frame edges, as a fraction of the frame height (default 0.05)"),
                    "safe_area": boolean("Keep it inside the title-safe area (a 10% margin), instead of `margin`"),
                    "fit": one_of(&layout::FITS, "keep its size (default); contain: fit inside the margins; cover: fill the frame, cropping; fill: stretch to the frame; width / height: `size` of the frame's width / height"),
                    "size": number("For fit width / height: the fraction of the frame, e.g. 0.15"),
                    "at": time("Timeline time inside the clip to place it at (default: its middle)"),
                    "reason": reason()
                }),
                &["clip"],
            ),
        ),
        spec(
            "animate",
            Some(Transform),
            "Propose a preset move as ordinary keyframes the user can edit, around the clip's own transform. *_in presets end at rest, *_out presets start at rest; slide_in_<side> enters from that side, slide_out_<side> leaves toward it. pulse, shake and drift play over `duration` (default: the whole clip).",
            object(
                json!({
                    "clip": string("Clip id"),
                    "preset": one_of(&layout::PRESETS, "The move"),
                    "duration": number("Seconds (default 0.4; loops: the clip)"),
                    "at": time("Timeline time it starts (default: the clip's start; *_out: so it ends with the clip)"),
                    "distance": number("Slides: how far, as a fraction of the frame (default 0.3)"),
                    "intensity": number("pulse, shake, drift: 1 = default strength"),
                    "reason": reason()
                }),
                &["clip", "preset"],
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
            "Propose primary correction on clips. Only the values given change; curves given replace that channel's curve.",
            object(
                grade_props(json!({ "clips": strings("Clip ids") })),
                &["clips"],
            ),
        ),
        spec(
            "set_timeline_grade",
            Some(Color),
            "Propose a grade for the whole timeline, applied over every media clip's own grade. Only the values given change.",
            object(grade_props(json!({})), &[]),
        ),
        spec(
            "set_curves",
            Some(Color),
            "Propose custom curves on clips (or the timeline grade when `clips` is omitted). Points are [input, output] in 0..1; channels given replace that curve, [] resets it.",
            object(
                json!({
                    "clips": strings("Clip ids (default: the timeline grade)"),
                    "master": curve("Master curve"),
                    "red": curve("Red curve"),
                    "green": curve("Green curve"),
                    "blue": curve("Blue curve"),
                    "reason": reason()
                }),
                &[],
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
            "Measure a reference frame and each clip's middle frame, then propose grades (lift, gain, gamma per channel, saturation) that bring the clips' levels to the reference's.",
            object(
                json!({
                    "reference": string("Clip id"),
                    "t": time("Timeline time of the reference frame (default: its middle)"),
                    "clips": strings("Clip ids to match"),
                    "reason": reason()
                }),
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
                json!({ "clip": string("Clip id"), "edge": one_of(edges, "Which edge"), "duration": time("Seconds or a time string"), "reason": reason() }),
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
        spec(
            "set_audio_effects",
            Some(Audio),
            "Propose a clip's (or a track's) whole audio effect chain, in order. Give `id` on an item to keep an existing effect.",
            object(
                json!({
                    "clip": string("Clip id"),
                    "track": string("Track name or id (track inserts, after clip effects)"),
                    "effects": { "type": "array", "items": audio_effect_item(), "description": "The chain, first applied first" },
                    "reason": reason()
                }),
                &["effects"],
            ),
        ),
        spec(
            "add_audio_effect",
            Some(Audio),
            "Propose one audio effect on a clip or a track.",
            object(
                json!({
                    "clip": string("Clip id"),
                    "track": string("Track name or id"),
                    "kind": one_of(&audio_kinds, "Processor"),
                    "params": params("Parameter name → number (list_fx kind=audio for names and ranges)"),
                    "index": number("Position in the chain (default: last)"),
                    "reason": reason()
                }),
                &["kind"],
            ),
        ),
        spec(
            "remove_audio_effect",
            Some(Audio),
            "Propose removing an audio effect from a clip or a track.",
            object(
                json!({ "clip": string("Clip id"), "track": string("Track name or id"), "effect": string("Audio effect id"), "reason": reason() }),
                &["effect"],
            ),
        ),
        // ---- effects, transitions, generators
        spec(
            "add_effect",
            Some(Effects),
            "Propose a video effect on clips' effect stacks (applied after the grade, before the transform). Returns each new effect's `@effect clip/effect` reference.",
            object(
                json!({
                    "clips": strings("Clip ids"),
                    "fx": string("Effect: built-in key, project id or `@fx` token"),
                    "params": params("Parameter name → value; omitted ones keep their defaults"),
                    "index": number("Position in the stack (default: last)"),
                    "enabled": boolean("Default true"),
                    "reason": reason()
                }),
                &["clips", "fx"],
            ),
        ),
        spec(
            "set_effect",
            Some(Effects),
            "Propose new parameter values (merged over the current ones) or enabled state for an effect on a clip.",
            object(
                json!({
                    "clip": string("Clip id"),
                    "effect": string("Effect id or `@effect clip/effect`"),
                    "params": params("Parameter name → value"),
                    "reset_params": boolean("Start from the defaults instead of merging"),
                    "enabled": boolean("Bypass with false"),
                    "reason": reason()
                }),
                &["clip", "effect"],
            ),
        ),
        spec(
            "remove_effect",
            Some(Effects),
            "Propose removing an effect from a clip.",
            object(
                json!({ "clip": string("Clip id"), "effect": string("Effect id"), "reason": reason() }),
                &["clip", "effect"],
            ),
        ),
        spec(
            "move_effect",
            Some(Effects),
            "Propose moving an effect within a clip's stack (0 = applied first).",
            object(
                json!({ "clip": string("Clip id"), "effect": string("Effect id"), "index": number("New position"), "reason": reason() }),
                &["clip", "effect", "index"],
            ),
        ),
        spec(
            "set_effect_keyframe",
            Some(Effects),
            "Propose a keyframe on a parameter of a clip effect, of a generator clip (`effect`: generator) or of a transition (`effect`: in or out). Values ease into the next keyframe.",
            object(
                json!({
                    "clip": string("Clip id"),
                    "effect": string("Effect id, `generator`, `in`, `out`, or an `@effect`/`@transition` token"),
                    "param": string("Parameter name"),
                    "at": time("Timeline time"),
                    "value": { "description": "Number, bool, [r, g, b, a], [x, y] or text, as the parameter takes" },
                    "ease": one_of(eases, "Easing into the next keyframe (default ease_in_out)"),
                    "reason": reason()
                }),
                &["clip", "effect", "param", "at", "value"],
            ),
        ),
        spec(
            "remove_effect_keyframe",
            Some(Effects),
            "Propose removing an effect parameter keyframe at a timeline time.",
            object(
                json!({
                    "clip": string("Clip id"),
                    "effect": string("Effect id, `generator`, `in` or `out`"),
                    "param": string("Parameter name"),
                    "at": time("Timeline time of the keyframe"),
                    "reason": reason()
                }),
                &["clip", "effect", "param", "at"],
            ),
        ),
        spec(
            "set_transition",
            Some(Effects),
            "Propose a transition at a clip's start (from the clip before it, or from nothing) or end. Without `fx`, changes the transition already there.",
            object(
                json!({
                    "clip": string("Clip id or `@transition clip/in|out`"),
                    "edge": one_of(edges, "start = transition in, end = transition out"),
                    "fx": string("Transition: built-in key (e.g. cross_dissolve), project id or `@fx` token"),
                    "duration": time("Length (default 0.5 s, or the current one)"),
                    "align": one_of(aligns, "Where it sits at a cut (default center)"),
                    "params": params("Parameter name → value"),
                    "reason": reason()
                }),
                &["clip"],
            ),
        ),
        spec(
            "remove_transition",
            Some(Effects),
            "Propose removing a clip's transition in or out.",
            object(
                json!({ "clip": string("Clip id or `@transition` token"), "edge": one_of(edges, "start or end"), "reason": reason() }),
                &["clip"],
            ),
        ),
        spec(
            "add_generator",
            Some(Effects),
            "Propose a generator clip (shader background or HTML motion graphic: title, lower third, callout) over a range. Without `track` it goes on the first title track (HTML) or overlay track (GLSL), created if missing.",
            object(
                json!({
                    "fx": string("Generator: built-in key, project id or `@fx` token"),
                    "range": range("Timeline range"),
                    "track": string("Video, overlay or title track name or id"),
                    "params": params("Parameter name → value"),
                    "reason": reason()
                }),
                &["fx", "range"],
            ),
        ),
        spec(
            "set_generator",
            Some(Effects),
            "Propose new parameter values (merged) for a generator clip, or swap its program.",
            object(
                json!({
                    "clip": string("Generator clip id"),
                    "fx": string("Another generator to use instead"),
                    "params": params("Parameter name → value"),
                    "reset_params": boolean("Start from the defaults instead of merging"),
                    "reason": reason()
                }),
                &["clip"],
            ),
        ),
        spec(
            "create_fx",
            Some(Effects),
            "Write a new effect, transition or generator for this project in GLSL (or HTML for generators). It is validated and compiled first; on errors nothing is saved and you get them line by line. On success it is proposed and usable by its id at once.",
            object(
                json!({
                    "name": string("Name in the library"),
                    "kind": one_of(fx_kinds, "What it is"),
                    "language": one_of(&["glsl", "html"], "Default glsl; html for generators only"),
                    "code": string("GLSL defining vec4 effect/transition/generate(vec2 uv), or an HTML document"),
                    "params": {
                        "type": "array",
                        "description": "Parameters: {name, type: float|int|bool|color|point|text|choice|image, default, label?, min?, max?, options?}",
                        "items": { "type": "object", "properties": { "name": { "type": "string" }, "type": { "type": "string" } }, "required": ["name", "type"] }
                    },
                    "category": string("Library group, e.g. Stylize"),
                    "description": string("One line"),
                    "reason": reason()
                }),
                &["name", "kind", "code"],
            ),
        ),
        spec(
            "update_fx",
            Some(Effects),
            "Change one of this project's FX programs (code, params, name…); validated and compiled like create_fx. Clips that use it follow.",
            object(
                json!({
                    "fx": string("Project program id or `@fx` token"),
                    "name": string("New name"),
                    "code": string("New code"),
                    "params": { "type": "array", "description": "Replaces the parameter list (same shape as create_fx)", "items": { "type": "object" } },
                    "category": string("Library group"),
                    "description": string("One line"),
                    "reason": reason()
                }),
                &["fx"],
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
