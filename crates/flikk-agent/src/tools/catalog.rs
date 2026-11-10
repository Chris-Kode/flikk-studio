//! One JSON schema per tool (PRD "Agent tool API"). Times are timeline
//! seconds; strings like "1:02.500", timecode "00:01:02:30" and frames
//! "f:3750" are accepted too.

use super::{depth, groups, layout, masks};
use flikk_core::audiofx::AudioFxKind;
use flikk_core::cloud;
use flikk_core::command::ToolCategory;
use flikk_core::model::{BlendMode, ClipColor, Format, Property};
use serde_json::{Value, json};

pub struct ToolSpec {
    pub name: &'static str,
    /// `None` for `ask_user`, `load_tools` and `run_tool`, which are always
    /// available.
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

/// A number kept inside `range`, which the description gives.
fn limited(desc: &str, range: std::ops::RangeInclusive<f32>) -> Value {
    number(&format!("{desc}, {} to {}", range.start(), range.end()))
}

fn color(desc: &str) -> Value {
    string(&format!("{desc}, #RRGGBB or #RRGGBBAA"))
}

/// Every caption style parameter, each optional: what is given changes.
fn caption_style_props(mut props: Value) -> Value {
    use flikk_core::memory::limits;
    let motion = |desc: &str| {
        json!({
            "type": "object",
            "description": desc,
            "properties": {
                "kind": one_of(&["none", "fade", "pop", "slide"], "pop grows in (shrinks out), slide rises in (sinks out)"),
                "duration": limited("Seconds", limits::MOTION),
            },
        })
    };
    let params = json!({
        "font": string("Font family installed on this computer, e.g. Inter, Montserrat"),
        "size": limited("Text size in px at 1080p", limits::SIZE),
        "bold": boolean("Bold"),
        "italic": boolean("Italic"),
        "uppercase": boolean("Show the text in capitals"),
        "color": color("Text fill"),
        "spacing": limited("Extra space after each character, px", limits::SPACING),
        "outline": limited("Stroke width, px (0 = none)", limits::OUTLINE),
        "outline_color": color("Stroke color"),
        "shadow": limited("Shadow distance, px (0 = none)", limits::SHADOW),
        "shadow_color": color("Shadow color; its alpha is the shadow's opacity"),
        "shadow_angle": limited("Degrees clockwise from the right: 45 casts down and right", limits::SHADOW_ANGLE),
        "background": {
            "type": "object",
            "description": "A box behind each line",
            "properties": {
                "enabled": boolean("Show the box"),
                "color": color("Box color"),
                "padding": limited("Space around the text, px", limits::PADDING),
            },
        },
        "position": one_of(&["bottom", "middle", "top"], "Where captions sit"),
        "offset_x": limited("Nudge right, % of the frame width", limits::OFFSET),
        "offset_y": limited("Nudge up, % of the frame height", limits::OFFSET),
        "highlight": one_of(&["none", "color", "box", "karaoke", "reveal"], "How the spoken word stands out: color tints it, box puts the highlight color behind it, karaoke tints words as spoken and keeps them, reveal shows words as they are spoken"),
        "highlight_color": color("Highlight color"),
        "intro": motion("How a caption enters"),
        "outro": motion("How a caption leaves"),
        "max_chars_per_line": limited("Longest line, characters", limits::LINE_LENGTH),
        "max_lines": number("Lines per caption, 1 to 3"),
    });
    if let (Some(props), Some(params)) = (props.as_object_mut(), params.as_object()) {
        props.extend(params.clone());
    }
    props
}

fn reason() -> Value {
    string("One line shown to the user with the change; cite the style rule that drove it, if any")
}

/// A few typical calls, as JSON Schema `examples`, for tools whose
/// arguments are easy to get wrong.
fn examples(mut schema: Value, examples: Value) -> Value {
    schema["examples"] = examples;
    schema
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
    let mut tools = vec![
        // ---- read project
        spec(
            "get_timeline",
            Some(ReadProject),
            "Tracks, clips and markers overlapping a range (default: whole timeline), including your pending proposals. Media clips give the media id; `media` maps each id to its file name.",
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
            "Media pool items, optionally in one bin, with whether each is analysed. view_media sees or hears one.",
            object(json!({ "bin": string("Bin name") }), &[]),
        ),
        spec(
            "find_sounds",
            Some(ReadProject),
            "Find music and sound effects to use, in the user's library (shared by every project) and this project's pool. `text` matches names, tags and what generated audio was asked to be, and how the audio sounds where it has been analysed; without it, lists them. search_sounds looks inside what is on the timeline instead.",
            object(
                json!({
                    "text": string("What it is or sounds like, in a few words"),
                    "scope": one_of(&["all", "library", "project"], "Where to look (default all)"),
                    "limit": number("Most sounds (default 12)")
                }),
                &[],
            ),
        ),
        spec(
            "get_clip",
            Some(ReadProject),
            "Everything about one clip: source, name, color, speed, enabled, transform (position, scale, stretch, rotation, skew, anchor, opacity, crop, flip, blend), keyframes, grade and curves, effects with their values and keyframes, transitions (and where they play), generator values, audio and audio effects. Values at their neutral default are left out.",
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
            "Transcript of a timeline range, in lines by speaker and pauses. With `word_times`, each word is `word@seconds`.",
            object(
                json!({
                    "range": range("Timeline range"),
                    "word_times": boolean("Give each word's start time, which `cut_words` takes. Ask for it on the stretch you are about to cut.")
                }),
                &["range"],
            ),
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
            "Gaps without speech of at least `min_s` seconds, as [start, end] pairs in timeline seconds.",
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
            "Music in a range: tags, tempo, bar length, downbeats in timeline seconds, and where it gets louder or softer (a drop, a breakdown).",
            object(
                json!({
                    "range": range("Timeline range (default: whole timeline)"),
                    "beats": boolean("Also list every beat. Cuts snap to beats with `align` without it.")
                }),
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
            "See a range: its sound script plus a collage of up to 30 frames spread evenly over it, each stamped with its timeline time. With `detail`, a denser collage of a short span (at most 20 s), for checking a cut or a moment.",
            object(
                json!({
                    "range": range("Timeline range"),
                    "detail": boolean("Denser frames over at most 20 s")
                }),
                &["range"],
            ),
        ),
        spec(
            "view_frame",
            Some(View),
            "One full frame of the program at a timeline time, captions and titles included. With `annotate`, every visible item gets a numbered box, listed with its clip. With `zoom`, only that region, magnified at full resolution under a grid labelled in frame pixels: look closely before you set a mask, pin a picture or check an edge. With `masks`, masked pictures are shown whole with what their masks cut away tinted red: check a mask's edge with it (zoom in too).",
            examples(
                object(
                    json!({
                        "t": time("Timeline seconds"),
                        "annotate": boolean("Box and number each visible item"),
                        "zoom": numbers(4, "[left, top, width, height] in frame pixels"),
                        "masks": boolean("Show what masks cut away, tinted, instead of hiding it")
                    }),
                    &["t"],
                ),
                json!([
                    { "t": 12.0, "annotate": true },
                    { "t": 12.0, "zoom": [760, 180, 420, 420] },
                    { "t": "0:08.000", "zoom": [780, 260, 520, 340] },
                    { "t": 12.0, "masks": true, "zoom": [600, 100, 700, 700] }
                ]),
            ),
        ),
        spec(
            "view_media",
            Some(View),
            "See or hear a media pool item on its own, whether or not it is on the timeline: never place media just to look at it. Times are the item's own seconds. A video: its sound script and a collage of up to 30 frames over `range` (default: all of it), denser with `detail` (at most 20 s); with `t`, one full frame, and `zoom` magnifies a region of it under a grid in the item's pixels. A picture: the whole picture, or `zoom` on part of it. Audio, or a video's sound with `audio`: its whole sound script (words, sounds, music, silences), and a spectrogram with words, sounds and beats marked when `range` is at most 30 s. An item not analysed yet is analysed first and the call waits for it.",
            examples(
                object(
                    json!({
                        "media": string("Media id from list_media, or an `@media` token"),
                        "range": range("Part of the item, in its own seconds (default: all of it)"),
                        "detail": boolean("Video: denser frames over at most 20 s"),
                        "t": time("Video: one frame at this second of the item"),
                        "zoom": numbers(4, "[left, top, width, height] in the item's pixels: a video frame at `t`, or a picture"),
                        "audio": boolean("Video: hear its sound instead of seeing frames")
                    }),
                    &["media"],
                ),
                json!([
                    { "media": "3f2a91c0" },
                    { "media": "3f2a91c0", "range": { "start": 4.0, "end": 9.0 }, "detail": true },
                    { "media": "3f2a91c0", "t": 6.5, "zoom": [640, 200, 480, 360] },
                    { "media": "3f2a91c0", "audio": true, "range": { "start": 0, "end": 12 } }
                ]),
            ),
        ),
        spec(
            super::web::TOOL,
            Some(View),
            "Look at a picture on the web: a PNG, JPEG, GIF or WebP address, such as an <img> src or a page's og:image (reading a page gives you its text only). With `add`, it also goes into the pool's Web Images bin to use like any other picture; add only what the user asked for.",
            object(
                json!({
                    "url": string("Address of the picture itself, not the page"),
                    "add": boolean("Also add it to the pool")
                }),
                &["url"],
            ),
        ),
        spec(
            "list_files",
            Some(Files),
            "List what is inside the files and folders the user attached to the chat, the only places on disk you may read. Like git sees them: ignored, hidden, dependency and build folders are left out. Paths are relative to their attached folder.",
            object(
                json!({
                    "path": string("An attached folder or a folder inside one (default: every attachment)"),
                    "depth": number("Levels of folders to go down (default 3)")
                }),
                &[],
            ),
        ),
        spec(
            "read_file",
            Some(Files),
            "Read a file inside what the user attached: text with line numbers, or a PNG, JPEG, GIF or WebP picture to look at. Long files come in parts; pass `offset` for the next.",
            object(
                json!({
                    "path": string("Path inside an attached folder, or a full path"),
                    "offset": number("First line to read, from 1"),
                    "limit": number("Lines to read (default 2000)")
                }),
                &["path"],
            ),
        ),
        spec(
            "search_files",
            Some(Files),
            "Search the text files inside what the user attached for a regex, line by line (`(?i)` ignores case). Gives path:line: text.",
            object(
                json!({
                    "pattern": string("Regular expression"),
                    "path": string("An attached folder, or a folder or file inside one (default: every attachment)"),
                    "glob": string("Only files matching this glob, such as *.css or src/**/*.ts")
                }),
                &["pattern"],
            ),
        ),
        spec(
            "copy_file",
            Some(Files),
            "Copy a file out of what the user attached into Flikk's own data, for when the project itself must use it (a font, a logo): the attachment can move or go away. Gives the copy's path. With `add_to_pool`, a picture also goes into the pool's Attached bin.",
            object(
                json!({
                    "path": string("Path inside an attached folder, or a full path"),
                    "add_to_pool": boolean("Also add the picture to the media pool")
                }),
                &["path"],
            ),
        ),
        spec(
            "get_layout",
            Some(View),
            "Where everything visible at a time is on screen, topmost first: each item's box around what it draws (pixels and fractions of the frame), its transform (values off their neutral default), whether it sits inside the title-safe area, and which items overlap. Use it before placing things so they don't cover each other or faces.",
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
            "Propose removing spoken words. Give each run by the `@` times of its first and last word from `get_transcript` with `word_times`. The cuts go in the pauses around the run, clear of the words kept, with a short audio fade; the pause before the run stays, the one after it goes. The result says where a cut has no pause to sit in.",
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
            "add_track",
            Some(EditTimeline),
            "Propose a new track. Name it for what it will hold (\"Interview\", \"B-roll\", \"Music\", \"SFX\") so the user can review the timeline at a glance. Returns its id.",
            object(
                json!({
                    "kind": one_of(&["video", "audio", "caption", "title", "overlay"], "What the track holds"),
                    "name": string("Unique track name (default: its code, e.g. V2)"),
                    "above": string("Put it right above this track (name or id)"),
                    "below": string("Put it right below this track (name or id)"),
                    "position": one_of(&["top", "bottom"], "Top or bottom of the stack"),
                    "reason": reason()
                }),
                &["kind"],
            ),
        ),
        spec(
            "rename_track",
            Some(EditTimeline),
            "Propose a new name for a track, saying what it holds.",
            object(
                json!({ "track": string("Track name or id"), "name": string("Unique new name"), "reason": reason() }),
                &["track", "name"],
            ),
        ),
        spec(
            "move_track",
            Some(EditTimeline),
            "Propose moving a track up or down the stack. Upper video tracks cover lower ones.",
            object(
                json!({
                    "track": string("Track name or id"),
                    "above": string("Put it right above this track (name or id)"),
                    "below": string("Put it right below this track (name or id)"),
                    "position": one_of(&["top", "bottom"], "Top or bottom of the stack"),
                    "reason": reason()
                }),
                &["track"],
            ),
        ),
        spec(
            "remove_track",
            Some(EditTimeline),
            "Propose removing an empty track. Move or delete its clips first.",
            object(
                json!({ "track": string("Track name or id"), "reason": reason() }),
                &["track"],
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
                    "track": string("Track name or id (default: first video or audio track, created if missing)"),
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
            "Propose retiming a media clip: 2 plays twice as fast (the clip gets shorter), 0.5 half speed. A ramp (or preset) changes the speed over the clip; the clip plays the same source, so its length follows unless keep_length. Slowed frames repeat, blend, or are made by optical flow. Retimes one clip: give a video's linked audio clip the same call.",
            object(
                json!({
                    "clip": string("Clip id"),
                    "speed": number("1 = normal, 0.01–100; on a ramped clip, ends the ramp. With preset: the base speed (default the clip's)"),
                    "ramp": curve("Speed ramp [[position 0..1 over the clip, speed], …], straight between points; [] ends it"),
                    "preset": one_of(&["ramp_in", "ramp_out", "hero", "bullet", "montage"], "Ready-made ramp: ramp_in slow→fast, ramp_out fast→slow, hero fast-slow-fast, bullet near-frozen middle, montage fast and slow by turns"),
                    "keep_length": boolean("Scale the ramp's speeds so the clip keeps its length"),
                    "interpolation": one_of(&["nearest", "blend", "optical_flow"], "Frames between source frames when slowed: repeat (default), blend, or optical flow (RIFE; blended until made or without the model)"),
                    "ramp_audio": one_of(&["stretch", "mute"], "Audio under a ramp: follows the speed, pitch kept (default), or muted"),
                    "ripple": boolean("Move later clips to follow the new length (default false: stop at the next clip)"),
                    "reason": reason()
                }),
                &["clip"],
            ),
        ),
        spec(
            "sync_audio",
            Some(EditTimeline),
            "Line recordings up by their sound (a second camera, a separate audio recorder) and propose it: each follower, with its linked sound, moves to where it sounds like the reference; clock drift is corrected by a tiny speed change. Returns each one's offset and confidence; one that doesn't match is left alone. Give `clips` on the timeline, or `media` to place pool items stacked in sync from `at`.",
            object(
                json!({
                    "clips": strings("Clip ids, two or more recordings (a video clip brings its linked sound)"),
                    "media": strings("Pool media ids to place instead of `clips`"),
                    "reference": string("Clip or media id that stays put (default: the clip on the lowest video track, or the first video)"),
                    "replace_camera_audio": boolean("Disable the cameras' sound so only the recorder's is heard (default false)"),
                    "at": time("With `media`: where the reference starts (default: the playhead)"),
                    "reason": reason()
                }),
                &[],
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
            "set_format",
            Some(EditTimeline),
            "Propose the sequence's frame size: a format, a resolution, or both (the other stays), or an exact `width` and `height`. Clip transforms are relative to the frame, so footage of another shape fits inside it with bars unless its `fit` is `cover`. To reframe for a new shape, pass `fill`, then look at the clips (`view_frame`) and pan each with `set_transform` `position` (keyframed with `at` where the subject moves) so the subject stays in shot.",
            object(
                json!({
                    "format": {
                        "type": "string",
                        "enum": Format::ALL.map(|f| f.name()),
                        "description": Format::ALL.map(|f| format!("{}: {}", f.name(), f.uses)).join("; ")
                    },
                    "resolution": number("Pixels on the short side: 720, 1080, 1440 or 2160"),
                    "width": number("Exact width, even"),
                    "height": number("Exact height, even"),
                    "fill": boolean("Also set every footage and photo clip on video tracks to cover the frame (`fit: cover`)"),
                    "reason": reason()
                }),
                &[],
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
            "Propose a keyframe on a clip property at a timeline time, or with `remove` take the one there away.",
            examples(
                object(
                    json!({
                        "clip": string("Clip id"),
                        "property": one_of(&properties, "Property; crop_* are fractions 0..1, volume is clip gain in dB"),
                        "at": time("Timeline seconds, inside the clip"),
                        "value": number("Value (not with `remove`)"),
                        "ease": one_of(eases, "Easing into the next keyframe (default ease_in_out)"),
                        "remove": boolean("Remove the keyframe at `at` instead"),
                        "reason": reason()
                    }),
                    &["clip", "property", "at"],
                ),
                json!([
                    { "clip": "3f9a1c2e", "property": "scale", "at": 12.0, "value": 1.0, "reason": "Start of the punch-in" },
                    { "clip": "3f9a1c2e", "property": "scale", "at": "0:13.500", "value": 1.25, "ease": "ease_out", "reason": "Punch in on the answer" },
                    { "clip": "3f9a1c2e", "property": "opacity", "at": 20.0, "remove": true, "reason": "Drop the extra fade key" }
                ]),
            ),
        ),
        spec(
            "set_transform",
            Some(Transform),
            "Propose clips' transform: pictures, overlays, generators, captions and titles alike. Only the values given change. Without `at` it sets the static transform (keyframed properties keep overriding it); with `at`, properties that have keyframes get one there and the rest change statically. Captions and titles ignore crop, flip, blend and corner pin (the render burns them in without them). With `group`, several clips move, scale and turn as one about the centre of what they draw together.",
            examples(
                object(
                    json!({
                        "clip": string("Clip id"),
                        "clips": strings("More clip ids, changed alike"),
                        "position": numbers(2, "[x, y] offset from where it rests (centred; captions where their style puts them), as a fraction of the frame: [0.25, 0] moves it a quarter of the frame width right"),
                        "scale": number("1 = fitted (captions: their style's size)"),
                        "stretch": numbers(2, "[x, y] per-axis scale on top of `scale`, 1 = none"),
                        "rotation": number("Degrees clockwise, about the anchor"),
                        "skew": numbers(2, "[x, y] slant in degrees, ±80: x leans vertical edges right going down, y leans horizontal edges down going right"),
                        "anchor": numbers(2, "[x, y] the point it scales, skews and rotates about, as a fraction of its size from its centre: [-0.5, -0.5] = top-left corner. It stays where it rests, so changing it shifts a scaled, rotated or skewed picture: set it first, then place it"),
                        "opacity": number("0..1"),
                        "crop": numbers(4, "[left, top, right, bottom], fractions 0..1 cut away"),
                        "flip": { "type": "array", "items": { "type": "boolean" }, "minItems": 2, "maxItems": 2, "description": "[horizontal, vertical]" },
                        "blend": one_of(&blends, "Composite mode over the tracks below"),
                        "fit": one_of(&["contain", "cover"], "How the picture rests in the frame before the rest applies: contain = whole picture inside with bars, cover = fills the frame, overflow cut (pan it with `position`); kept when the format changes"),
                        "corner_pin": { "type": "array", "items": numbers(2, "[x, y]"), "minItems": 4, "maxItems": 4, "description": "How far each corner moves, [top-left, top-right, bottom-right, bottom-left], as [x, y] fractions of the picture's size before the rest of the transform; the picture is drawn in perspective onto the quad they make. All [0, 0] = none. To pin onto points of the frame, use place with `pin_to`" },
                        "relative": boolean("Position, rotation, skew and anchor add to the current values; scale, stretch and opacity multiply them"),
                        "group": boolean("Move `clips` as one group at `at` (default: the middle of the time they share): position moves the group by a fraction of the frame, scale multiplies its size, rotation turns it, about the centre of their bounds together; each clip's own transform changes to match. Only position, scale and rotation apply"),
                        "at": time("Timeline time to set it at, keyframe-aware (see above)"),
                        "reason": reason()
                    }),
                    &[],
                ),
                json!([
                    { "clip": "9d4e6b10", "scale": 0.4, "position": [0.28, -0.26], "reason": "Picture-in-picture, top right" },
                    { "clip": "9d4e6b10", "position": [-0.02, 0], "relative": true, "reason": "Nudge left" },
                    { "clip": "9d4e6b10", "anchor": [0.5, 0.5], "rotation": 8, "reason": "Tilt about its bottom-right corner" },
                    { "clips": ["5be20f71", "c03d8a9e"], "group": true, "scale": 0.8, "reason": "Shrink the logo and its label together" }
                ]),
            ),
        ),
        spec(
            "place",
            Some(Transform),
            "Propose putting a clip somewhere by meaning: align what it draws (its opaque part: a logo's pixels, a lower third inside its frame, a caption's box) to a corner, edge or the centre, a margin in from the frame edges, and size it. Several `clips` are placed as one group: their bounds together are aligned and sized, keeping their arrangement. Keyframe-aware at `at`. Check the result with get_layout or view_frame.",
            examples(
                object(
                    json!({
                        "clip": string("Clip id"),
                        "clips": strings("Clip ids placed together as one group (with or instead of `clip`)"),
                        "pin_to": { "type": "array", "items": numbers(2, "[x, y] frame pixels"), "minItems": 4, "maxItems": 4, "description": "Corner pin instead of aligning: put the whole picture's corners (top-left, top-right, bottom-right, bottom-left) on these frame points, in perspective, e.g. onto a screen or sign seen in the shot. One picture clip; not captions or titles" },
                        "align": one_of(&layout::ALIGNS, "Where it goes (default center)"),
                        "margin": number("Distance from the frame edges, as a fraction of the frame height (default 0.05)"),
                        "safe_area": boolean("Keep it inside the title-safe area (a 10% margin), instead of `margin`"),
                        "fit": one_of(&layout::FITS, "keep its size (default); contain: fit inside the margins; cover: fill the frame, cropping; fill: stretch to the frame; width / height: `size` of the frame's width / height"),
                        "size": number("For fit width / height: the fraction of the frame, e.g. 0.15"),
                        "at": time("Timeline time inside the clip to place it at (default: its middle; a group: the middle of the time its clips share)"),
                        "reason": reason()
                    }),
                    &[],
                ),
                json!([
                    { "clip": "5be20f71", "align": "top_right", "fit": "width", "size": 0.12, "reason": "Logo in the corner, small" },
                    { "clips": ["5be20f71", "c03d8a9e"], "align": "bottom_left", "safe_area": true, "reason": "Name and title together, inside title-safe" },
                    { "clip": "9d4e6b10", "fit": "cover", "reason": "Fill the vertical frame" },
                    { "clip": "9d4e6b10", "pin_to": [[812, 301], [1240, 288], [1252, 560], [806, 571]], "reason": "Put the slide on the laptop screen" }
                ]),
            ),
        ),
        spec(
            "set_mask",
            Some(Transform),
            "Propose a mask on a picture clip (video, image, overlay, generator), a change to one given by `mask`, or with `remove` take `mask` (or `all` masks) away. A mask keeps (or with `invert` cuts away) the inside of a shape, applied before the clip is placed and blended. Shapes live in the picture's own coordinates, so they follow its transform. Several masks combine in order: add unites, subtract cuts away, intersect keeps the overlap. With `at`, the shape is keyframed there (key it at two times to animate it); without, the static shape changes. Only the values given change. Not for captions or titles.\n\nA `subject` shape (video and image clips from the pool) keeps an object a segmentation model finds from `include` points on it, `exclude` points off it and a `box` around it, on the frame at `at` (default: the clip's middle); its edge is exact, so prefer it for objects and for one person among several. A `person` shape keeps every person in every frame with soft hair edges and no prompt (`edge` 0..1 eases the edge onto the picture's); `remove_background` adds it to several clips at once. The result says what it kept; `exclude` what it took by mistake, `include` what it missed (each list given replaces that frame's own). `track: true` follows the object through the clip in the background; a prompt at another `at` corrects it where it drifts.\n\nA `depth` shape (video and image clips from the pool) keeps what lies between `near` and `far` from the camera, by the picture's depth map (relative distance: 0 nearest, 1 farthest), fading over `softness`: `near: 0.5, far: 1` keeps the background, `near: 0, far: 0.4` the foreground. To grade or add an effect to one of them only, put a copy of the clip on the track above with the mask and change the copy.",
            examples(
                object(
                    json!({
                        "clip": string("Clip id"),
                        "mask": string("Mask id to change (from get_clip); omit to add a new mask"),
                        "shape": {
                            "type": "object",
                            "description": "The outline. rectangle and ellipse: `center` [x, y] and `size` [w, h], `rotation` degrees clockwise, rectangle `roundness` 0..1; path: `points` [[x, y], …] (three or more), joined by straight lines, or with `smooth` by a curve through them (for faces and other round shapes; it can swing past the points); subject: `include` [[x, y], …] on the object, `exclude` [[x, y], …] off it, `box` [left, top, right, bottom] around it (null removes it); depth: `near`, `far`, `softness` in distance (0 nearest, 1 farthest). Coordinates are fractions of the picture (0..1 across it), or frame pixels with `space: frame`",
                            "properties": {
                                "type": { "type": "string", "enum": masks::SHAPES },
                                "center": numbers(2, "[x, y]"),
                                "size": numbers(2, "[width, height]"),
                                "rotation": number("Degrees clockwise"),
                                "roundness": number("Rectangle corners, 0..1"),
                                "points": { "type": "array", "items": numbers(2, "[x, y]") },
                                "smooth": boolean("A curve through the points instead of straight lines (default false)"),
                                "include": { "type": "array", "items": numbers(2, "[x, y]"), "description": "Subject: points on the object" },
                                "exclude": { "type": "array", "items": numbers(2, "[x, y]"), "description": "Subject: points not on it" },
                                "box": { "type": ["array", "null"], "items": { "type": "number" }, "description": "Subject: [left, top, right, bottom] around it" },
                                "edge": number("Person: 0..1, how much the edge follows the picture's own (hair); default 0.5"),
                                "near": number("Depth: nearest distance kept, 0 (nearest) to 1 (farthest)"),
                                "far": number("Depth: farthest distance kept"),
                                "softness": number("Depth: fade past each end, in distance (e.g. 0.05)")
                            }
                        },
                        "space": one_of(&["picture", "frame"], "picture (default): fractions of the picture; frame: pixels of the frame at `at` (or the clip's middle), mapped onto the picture where it is then"),
                        "mode": one_of(&masks::MODES, "How it combines with the masks before it (default add)"),
                        "invert": boolean("Cut the shape away instead of keeping it"),
                        "feather": number("Soft edge width, as a fraction of the picture's height (e.g. 0.03)"),
                        "expansion": number("Grow (or shrink, below 0) the edge, as a fraction of the picture's height"),
                        "opacity": number("0..1"),
                        "enabled": boolean("Off keeps the mask but ignores it"),
                        "at": time("Timeline time to keyframe the shape at; for a subject, the frame its points are on"),
                        "track": boolean("Subject: follow it through the clip"),
                        "refine": boolean("Subject: ease its edge onto the picture's (hair); default on"),
                        "remove": boolean("Remove `mask` instead"),
                        "all": boolean("With `remove`: every mask on the clip"),
                        "reason": reason()
                    }),
                    &["clip"],
                ),
                json!([
                    { "clip": "3f9a1c2e", "shape": { "type": "ellipse", "center": [0.5, 0.42], "size": [0.35, 0.5] }, "feather": 0.04, "reason": "Spotlight the speaker's face" },
                    { "clip": "3f9a1c2e", "mask": "a71c09d4", "shape": { "center": [0.62, 0.42] }, "at": "0:08.000", "reason": "Follow the face as she moves" },
                    { "clip": "3f9a1c2e", "shape": { "type": "rectangle", "center": [960, 540], "size": [1920, 140] }, "space": "frame", "invert": true, "reason": "Hide the ticker" },
                    { "clip": "3f9a1c2e", "shape": { "type": "path", "points": [[1210, 180], [1330, 205], [1350, 420], [1240, 470], [1150, 330]], "smooth": true }, "space": "frame", "reason": "Cut out her head" },
                    { "clip": "3f9a1c2e", "shape": { "type": "subject", "include": [[1240, 520]] }, "space": "frame", "at": "0:04.000", "track": true, "reason": "Keep the speaker, cut the background" },
                    { "clip": "3f9a1c2e", "mask": "c41e7a02", "shape": { "exclude": [[1420, 610]] }, "space": "frame", "at": "0:04.000", "reason": "Leave out the chair it took" },
                    { "clip": "3f9a1c2e", "shape": { "type": "subject", "box": [880, 120, 1460, 1080] }, "space": "frame", "invert": true, "track": true, "reason": "Everything but the dancer, for a background effect" },
                    { "clip": "3f9a1c2e", "remove": true, "all": true, "reason": "Back to the full picture" }
                ]),
            ),
        ),
        spec(
            "remove_background",
            Some(Transform),
            "Propose cutting the background away from behind the people in video and image clips, no green screen: each clip gets a person mask (a matting model finds every person in every frame, hair edges soft), or its own is changed. `keep: background` cuts the people away instead. Layers under the clip show through; the mask combines with the clip's other masks, effects and blend mode. With `restore`, the clips' person masks are taken away. People are found in the background as frames are shown; renders wait for them. For one object, or one person among several, use set_mask with a `subject` shape.",
            examples(
                object(
                    json!({
                        "clips": strings("Clip ids"),
                        "keep": one_of(&["people", "background"], "What stays (default people)"),
                        "edge": number("0..1, how much the edge follows the picture's own, for hair (default 0.5)"),
                        "feather": number("Soft edge width, as a fraction of the picture's height (e.g. 0.01)"),
                        "expansion": number("Grow (or shrink, below 0) the edge, as a fraction of the picture's height (e.g. -0.005 to trim a halo)"),
                        "restore": boolean("Take the clips' person masks away"),
                        "reason": reason()
                    }),
                    &["clips"],
                ),
                json!([
                    { "clips": ["3f9a1c2e"], "reason": "Put the presenter over the slides" },
                    { "clips": ["3f9a1c2e", "8c21d0b7"], "expansion": -0.004, "reason": "Trim the light halo round her hair" },
                    { "clips": ["3f9a1c2e"], "keep": "background", "reason": "Blur only the background: the copy above keeps the person" },
                    { "clips": ["3f9a1c2e"], "restore": true, "reason": "Back to the full picture" }
                ]),
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
                    "track": string("Caption track name or id (default: first caption track, created if missing)"),
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
        spec(
            "list_caption_styles",
            Some(ReadProject),
            "The style profile's caption styles with every parameter, the caption tracks lettered in each, the one new captions use, and the ready-made looks. Read it before creating or changing a style.",
            object(json!({}), &[]),
        ),
        spec(
            "set_caption_style",
            Some(Captions),
            "Propose lettering a caption track's captions in one of the profile's caption styles. The first caption track's style is also the one new captions use.",
            object(
                json!({
                    "track": string("Caption track name or id"),
                    "style": string("Caption style name"),
                    "reason": reason()
                }),
                &["track", "style"],
            ),
        ),
        spec(
            "create_caption_style",
            Some(CaptionStyles),
            "Add a caption style to the style profile, starting from `from` (a style or a ready-made look; default: the plain default style) with the parameters given changed. Saved once the user approves (at once if they allow it); then letter a track in it with set_caption_style.",
            object(
                caption_style_props(json!({
                    "name": string("Name of the new style, not used by another"),
                    "from": string("Style or look to start from, e.g. Neon, Karaoke"),
                })),
                &["name"],
            ),
        ),
        spec(
            "edit_caption_style",
            Some(CaptionStyles),
            "Change a caption style of the style profile: every caption track lettered in it changes. `look` dresses it in a ready-made look first (keeping its name, placement and line rules), then the parameters given change; `name` renames it, and the tracks follow. Saved once the user approves, or at once if they allow it.",
            object(
                caption_style_props(json!({
                    "style": string("The style to change"),
                    "name": string("New name"),
                    "look": string("Ready-made look to apply first"),
                })),
                &["style"],
            ),
        ),
        spec(
            "delete_caption_style",
            Some(CaptionStyles),
            "Remove a caption style from the style profile; tracks lettered in it fall back to the first style. The last style stays. Removed once the user approves, or at once if they allow it.",
            object(
                json!({ "style": string("The style to remove") }),
                &["style"],
            ),
        ),
        // ---- color
        spec(
            "set_primary",
            Some(Color),
            "Propose primary correction on clips, or without `clips` on the timeline grade (applied over every media clip's own grade). Only the values given change; curves given replace that channel's curve.",
            object(
                grade_props(json!({ "clips": strings("Clip ids (default: the timeline grade)") })),
                &[],
            ),
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
            "set_tone_map",
            Some(Color),
            "Propose how an HDR media item (`list_media` shows `hdr`) is mapped to SDR Rec.709, for every clip of it; the viewer and the export follow. `auto` (the default) tone maps it; `off` shows the raw signal, grey and washed out. `exposure` brightens or darkens before the curve; `peak` is the brightest the source is taken to get (lower keeps more contrast and clips highlights). Omitted fields keep their value.",
            object(
                json!({
                    "media": string("Media id"),
                    "mode": one_of(&["auto", "off"], "Tone mapping"),
                    "exposure": number("Stops, -3..3, 0 = neutral"),
                    "peak": number("Source peak in nits, 200..10000; 0 = from the file's metadata"),
                    "reason": reason()
                }),
                &["media"],
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
            "fit_music",
            Some(Audio),
            "Propose a song fitted to a length so it ends on its real ending exactly when wanted: bars are skipped or repeated where they sound alike, on phrase boundaries, with short crossfades. Default target: the end of the other tracks. Returns each skip and loop. Needs the song analysed.",
            object(
                json!({
                    "clip": string("Music clip id on an audio track (default: the only one)"),
                    "end": time("Timeline time the song should end at"),
                    "duration": time("Length from the clip's start"),
                    "range": range("Timeline range the song should fill, starting at its start"),
                    "loops": boolean("Repeat sections to make it longer (default true)"),
                    "crossfade_ms": number("Crossfade at each edit (default 30)"),
                    "reason": reason()
                }),
                &[],
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
                json!({ "media": string("Media id"), "at": time("Timeline seconds"), "track": string("Audio track (default: last unlocked audio track, created if missing)"), "reason": reason() }),
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
            "Propose new parameter values (merged over the current ones) or enabled state for an effect on a clip; or, on their own, move it in the stack with `index` or take it off with `remove`.",
            object(
                json!({
                    "clip": string("Clip id"),
                    "effect": string("Effect id or `@effect clip/effect`"),
                    "params": params("Parameter name → value"),
                    "reset_params": boolean("Start from the defaults instead of merging"),
                    "enabled": boolean("Bypass with false"),
                    "index": number("Move it to this position (0 = applied first)"),
                    "remove": boolean("Remove the effect"),
                    "reason": reason()
                }),
                &["clip", "effect"],
            ),
        ),
        spec(
            "set_effect_keyframe",
            Some(Effects),
            "Propose a keyframe on a parameter of a clip effect, of a generator clip (`effect`: generator) or of a transition (`effect`: in or out), or with `remove` take the one there away. Values ease into the next keyframe.",
            examples(
                object(
                    json!({
                        "clip": string("Clip id"),
                        "effect": string("Effect id, `generator`, `in`, `out`, or an `@effect`/`@transition` token"),
                        "param": string("Parameter name"),
                        "at": time("Timeline time"),
                        "value": { "description": "Number, bool, [r, g, b, a], [x, y] or text, as the parameter takes (not with `remove`)" },
                        "ease": one_of(eases, "Easing into the next keyframe (default ease_in_out)"),
                        "remove": boolean("Remove the keyframe at `at` instead"),
                        "reason": reason()
                    }),
                    &["clip", "effect", "param", "at"],
                ),
                json!([
                    { "clip": "3f9a1c2e", "effect": "@effect 3f9a1c2e/e2b7c419", "param": "amount", "at": 4.0, "value": 0.0, "reason": "Blur starts clear" },
                    { "clip": "7a01c5d3", "effect": "generator", "param": "accent", "at": "0:02.000", "value": [1.0, 0.6, 0.1, 1.0], "reason": "Accent warms up" },
                    { "clip": "3f9a1c2e", "effect": "in", "param": "softness", "at": 10.2, "remove": true, "reason": "Drop the extra key" }
                ]),
            ),
        ),
        spec(
            "set_transition",
            Some(Effects),
            "Propose a transition at a clip's start (from the clip before it, or from nothing) or end. Without `fx`, changes the transition already there; with `remove`, takes it away.",
            object(
                json!({
                    "clip": string("Clip id or `@transition clip/in|out`"),
                    "edge": one_of(edges, "start = transition in, end = transition out"),
                    "fx": string("Transition: built-in key (e.g. cross_dissolve), project id or `@fx` token"),
                    "duration": time("Length (default 0.5 s, or the current one)"),
                    "align": one_of(aligns, "Where it sits at a cut (default center)"),
                    "params": params("Parameter name → value"),
                    "remove": boolean("Remove the transition at `edge`"),
                    "reason": reason()
                }),
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
            "use_library_fx",
            Some(Effects),
            "Copy a program from the user's library (list_fx `library`) into this project; then use it by the project id returned. A program copied before is reused.",
            object(
                json!({
                    "fx": string("Library id or name"),
                    "reason": reason()
                }),
                &["fx"],
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
            "list_skills",
            Some(Skills),
            "The user's skills (slash commands they run from the chat), with their instructions.",
            object(json!({}), &[]),
        ),
        spec(
            "list_styles",
            Some(Skills),
            "The user's styles (standing editing instructions picked in the chat), with their instructions; `in_chat` marks the one in use.",
            object(json!({}), &[]),
        ),
        spec(
            "save_skill",
            Some(Skills),
            "Create a skill the user runs as /name, or replace the one with that name. Text typed after /name reaches you as the user's input.",
            object(
                json!({
                    "name": string("Slash name, lowercase-with-dashes"),
                    "description": string("One line for the / list"),
                    "instructions": string("What to do when the skill runs, as steps")
                }),
                &["name", "instructions"],
            ),
        ),
        spec(
            "save_style",
            Some(Skills),
            "Create a style, or replace the one with that name: how edits should feel (pace, music, captions, zooms), applied to every message while it is picked.",
            object(
                json!({
                    "name": string("Short name, e.g. Documentary"),
                    "instructions": string("The style's rules, as instructions")
                }),
                &["name", "instructions"],
            ),
        ),
        spec(
            "delete_skill",
            Some(Skills),
            "Ask the user to delete one of their skills.",
            object(json!({ "name": string("Slash name") }), &["name"]),
        ),
        spec(
            "delete_style",
            Some(Skills),
            "Ask the user to delete one of their styles.",
            object(json!({ "name": string("Style name") }), &["name"]),
        ),
        spec(
            "save_sound_to_library",
            Some(ProjectMemory),
            "Ask the user to approve saving audio from the pool to their library, so other projects can use it.",
            object(json!({ "media": string("Media id") }), &["media"]),
        ),
        spec(
            "save_fx_to_library",
            Some(ProjectMemory),
            "Ask the user to approve saving one of this project's FX programs to their library, so other projects can use it.",
            object(
                json!({ "fx": string("Project id or `@fx` token") }),
                &["fx"],
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
                json!({ "preset": string("Preset name, e.g. youtube_1080p"), "range": range("Timeline range (default: whole timeline)"), "encoder": one_of(&["auto", "hardware", "software"], "H.264/HEVC encoder (default: the user's choice in Deliver)") }),
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
        spec(
            "use_library_sound",
            Some(Audio),
            "Add a sound from the user's library (find_sounds) to the pool's Library bin, and propose it on an audio track with `at`. A sound the project has is reused.",
            object(
                json!({
                    "sound": string("Library id or name"),
                    "at": time("Propose it on the timeline from here; leave out to only add it to the pool"),
                    "track": string("Audio track id (default: one free over its length, else a new one)"),
                    "reason": reason()
                }),
                &["sound"],
            ),
        ),
        // ---- generate (credits: approved each time unless allowed)
        spec(
            "generate_audio",
            Some(Generate),
            "Make music, a sound effect or a voiceover with the provider the user set up. It lands in the pool's Music Generated, Sound Effects Generated or Voiceover Generated bin; with `at` or `range` it is also proposed on an audio track. It costs the user credits: unless they allowed it, each call waits for their approval and then returns its media id. Read the range first (get_music, get_audio, view_range) and describe what it needs; naming artists or songs is refused. A voiceover needs a `voice` from find_voices and runs as long as its words take, about 1,000 characters a minute.",
            object(
                json!({
                    "kind": one_of(&["music", "sound_effect", "voiceover"], "What to make"),
                    "prompt": string("Music and sound in English. Music: genre, mood, instruments, tempo, how it builds. Sound: what is heard, how close, the space around it. Voiceover: the script, in the language it is read in, up to 10,000 characters, with delivery written in as the system prompt says"),
                    "range": range("The timeline range it is for: sets its length, and where it is proposed"),
                    "duration": number("Seconds, when there is no `range`. Music 3–300, sound 0.5–30; left out, the model picks"),
                    "sections": {
                        "type": "array",
                        "description": "Music only: parts with lengths of their own, so it changes on cue (an intro up to the first cut, the drop on the reveal). 3–120 s each, 3–300 s together; replaces `duration`",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": string("\"Intro\", \"Build\", \"Drop\""),
                                "duration": number("Seconds"),
                                "styles": strings("Short English descriptors for this part"),
                                "avoid": strings("Styles to keep out"),
                                "lyrics": strings("Sung lines, for music with vocals")
                            },
                            "required": ["duration", "styles"]
                        }
                    },
                    "instrumental": boolean("Music without vocals (default true)"),
                    "loop": boolean("A sound effect made to repeat seamlessly (ambience, a hum)"),
                    "voice": string("Voiceover: the voice id from find_voices"),
                    "voice_name": string("Voiceover: the voice's name, for the approval card"),
                    "stability": number("Voiceover, 0–1 (default 0.5): lower reads with more emotional range, higher more evenly"),
                    "similarity": number("Voiceover, 0–1 (default 0.75): how closely it keeps to the original voice"),
                    "at": time("Propose it on the timeline from here (default: the start of `range`); leave out both to only add it to the pool"),
                    "track": string("Audio track to put it on (default: one with nothing there, else a new one)")
                }),
                &["kind", "prompt"],
            ),
        ),
        spec(
            "generate_video",
            Some(Generate),
            "Make a video with the provider and model the user set up (on fal: Seedance, Kling, MiniMax, Wan, Happy Horse, Gemini Omni Flash, FLUX or LTX; on Pika: Pika H3 Flex, Pika 2.5, Seedance, Kling, MiniMax, Hailuo, Wan, Happy Horse, Gemini Omni Flash, FLUX or Grok Imagine): from the prompt alone, from a first frame (a picture in the pool, or the program at a time) and optionally a last one, or after references (pictures, clips and sounds in the pool: a character, a style, a motion, a beat). It lands in the pool's Video Generated bin; with `at` or `range` it is also proposed on a video track, its sound on an audio track. It costs the user money and takes minutes: unless they allowed it, each call waits for their approval and then returns its media id. Lengths, aspect ratios, resolutions, end frames and references depend on the model the user picked; a request it can't make is refused with its limits before anything is spent.",
            object(
                json!({
                    "prompt": string("What happens, shot by shot if it changes: subject and action, camera (\"slow dolly in\", \"handheld\"), lens and light, style. With references, name them by kind and order: @Image1, @Video1, @Audio1 (the first picture, clip and sound in `references`), whatever the model; they are renamed for it"),
                    "range": range("The timeline range it is for: sets its length (whole seconds), and where it is proposed"),
                    "duration": number("Whole seconds, when there is no `range`; left out, the model picks"),
                    "aspect": one_of(&["16:9", "9:16", "1:1", "4:3", "3:4", "21:9", "9:21", "2:1", "5:4", "4:5"], "Shape (default: the model picks, from the first frame when there is one); match the sequence"),
                    "resolution": one_of(&["360p", "480p", "720p", "768p", "1080p", "4k"], "Default: the one the user set in Settings, else the model's"),
                    "first_frame": string("Media id of a picture in the pool to start on"),
                    "first_frame_at": time("Or start on the program's frame at this time (to continue a shot)"),
                    "last_frame": string("Media id of a picture to end on; needs a first frame"),
                    "last_frame_at": time("Or end on the program's frame at this time"),
                    "references": strings("Media ids of pictures, clips and sounds in the pool for it to follow; not with a first frame"),
                    "audio": boolean("Make sound with it, where the model makes sound (default true). The user can turn video sound off in Settings; then it is made silent whatever this says"),
                    "at": time("Propose it on the timeline from here (default: the start of `range`); leave out both to only add it to the pool"),
                    "track": string("Video track to put it on (default: one with nothing there, else a new one)")
                }),
                &["prompt"],
            ),
        ),
        spec(
            super::upscale::TOOL,
            Some(Generate),
            "Make a video in the pool larger with the upscale provider and model the user set up (on fal: Topaz Precision or Starlight, SeedVR2, Crystal, FLUX Video Upscale or Gemini Omni Flash; on Pika: Topaz Proteus, Starlight Precise or Fast, or Pika H3 Flex Upscale for videos of 768p or less). The picture is rebuilt at the resolution asked for; the sound is the source's. It lands in the pool's Video Upscaled bin; with `replace` it is also proposed in place of the source in its clips on picture tracks (same source times; sound clips keep the source). It costs the user money and takes minutes: unless they allowed it, each call waits for their approval and then returns its media id. Offer it when footage or a generated draft is small next to the sequence (list_media gives sizes), or when the user asks for sharper or higher resolution video. Resolutions, the longest and largest video and how much larger it can make one depend on the model; a request it can't make is refused with why before anything is spent.",
            object(
                json!({
                    "media": string("Media id of a video in the pool"),
                    "resolution": one_of(&["720p", "1080p", "2k", "4k"], "Lines on the short side (default: 1080p where the model can make it, else the smallest above the video's own)"),
                    "replace": boolean("Propose the upscaled video in place of this one in its clips on picture tracks (default false: it only goes to the pool)")
                }),
                &["media"],
            ),
        ),
        spec(
            super::image::TOOL,
            Some(Generate),
            "Make a picture: a still, a background, a title card, a texture, a cutout, or an edit of pictures in the pool or frames of the program (restyle a shot, change what is in it, combine a character with a place). It is made by the image provider and model the user set up in Settings → Integrations (on fal: Nano Banana, GPT Image, FLUX 3, Seedream, Ideogram, Grok Imagine, Qwen Image, Krea or Recraft; on Pika: Nano Banana, GPT Image, Seedream, Ideogram, Grok Imagine, Muse or Recraft), else with the user's ChatGPT plan (GPT Image, through their Codex sign-in), which makes pictures from a description only. It lands in the pool's Images Generated bin, to use like any other picture. Each one costs the user money or counts against their plan: make one only when the user asks for a picture. Unless they allowed generating, each call waits for their approval and then returns its media id. It takes up to a couple of minutes. Shapes, resolutions, how many pictures it follows and transparency depend on the model the user picked; a request it can't make is refused with its limits before anything is spent.",
            object(
                json!({
                    "prompt": string("What to make: subject, composition, style and light, and any words that appear in it, exactly as written. With pictures to follow, name them by order: @Image1, @Image2 (`references` first, then `frames_at`), and say what to keep and what to change. With the ChatGPT plan, also say its shape (wide 16:9, portrait 9:16, square)"),
                    "aspect": one_of(&["16:9", "9:16", "1:1", "4:3", "3:4", "3:2", "2:3", "5:4", "4:5", "21:9", "2:1", "1:2"], "Shape (default: the model picks, from the first picture when there is one); match the sequence for a full-frame still. Not with the ChatGPT plan"),
                    "resolution": one_of(&["1K", "1.5K", "2K", "3K", "4K"], "Default: the model's. Not with the ChatGPT plan"),
                    "references": strings("Media ids of pictures in the pool to edit or follow; not with the ChatGPT plan"),
                    "frames_at": {
                        "type": "array",
                        "items": { "type": ["number", "string"] },
                        "description": "Times of the program to grab as pictures to edit or follow (after `references`); not with the ChatGPT plan"
                    },
                    "transparent": boolean("A transparent background, for a cutout or an overlay (default: opaque), where the model makes one")
                }),
                &["prompt"],
            ),
        ),
        spec(
            "replace_words",
            Some(Generate),
            "Fix misspoken or missing words: the words from `from` to `to` are said again as `text`, in the speaker's own voice, and cut in where they were (the cuts sit in the pauses either side; what follows moves by the difference, and the picture over them is held or cut). To add a missing word, replace the word next to it with itself and the new one. The voice is the one the user linked to that speaker, or `voice`. It costs the user credits: unless they allowed it, each call waits for their approval. Proposed in your change set; the new words are in the Voiceover Generated bin.",
            object(
                json!({
                    "from": time("Start of the first word to replace, as the transcript gives it"),
                    "to": time("Start of the last word to replace (default: just the first)"),
                    "words": string("The words replaced, as the transcript has them, to check"),
                    "text": string("What is said instead, as the speaker would say it, with punctuation"),
                    "voice": string("A voice id from find_voices, when none is linked to the speaker"),
                    "voice_name": string("Its name, for the approval card"),
                    "reason": reason()
                }),
                &["from", "text"],
            ),
        ),
        spec(
            "find_voices",
            Some(ReadProject),
            "List the voices that can read a voiceover with the provider the user set up: their ids, names and what they are like, the ones in the user's library first (with how they read best), then ones they made. With `library`, search the provider's shared Voice Library instead; a shared voice reads only once add_voice adds it. Free. Pick one for generate_audio; for several speakers, make a clip per speaker, each with its own voice.",
            object(
                json!({
                    "text": string("Words the voice must have: \"calm british narration\". Left out, lists them"),
                    "library": boolean("Search the provider's shared Voice Library rather than the account"),
                    "gender": one_of(&cloud::VOICE_GENDERS, "Only voices of this gender"),
                    "age": one_of(&cloud::VOICE_AGES, "Only voices of this age"),
                    "language": string("Only voices speaking this language: ISO 639-1, like \"es\""),
                    "use_case": one_of(&cloud::VOICE_USES, "Only voices made for this"),
                    "limit": number("Most voices (default 20)")
                }),
                &[],
            ),
        ),
        spec(
            "add_voice",
            Some(Generate),
            "Add a voice found with find_voices `library: true` to the user's account and library, to read voiceovers with. It takes one of their plan's voice slots: unless they allowed generating, it waits for their approval and then returns the voice id.",
            object(
                json!({
                    "voice": string("The shared voice's id from find_voices"),
                    "owner": string("Its `owner` from find_voices"),
                    "name": string("What to call it: its name, unless the user named it")
                }),
                &["voice", "owner", "name"],
            ),
        ),
        spec(
            "design_voice",
            Some(Generate),
            "Make a new voice from a description, kept on the user's account and in their library, to read voiceovers with. Spends credits and a voice slot: only when neither the account nor the shared Voice Library (find_voices) has one that fits. Unless they allowed generating, it waits while the user hears the takes and keeps one, then returns the id of the voice they kept.",
            object(
                json!({
                    "description": string("20–1,000 characters: age, gender, accent, tone, pace and texture, like \"A calm woman in her 60s with a soft Irish accent, warm and unhurried, slightly breathy\". No real person's name"),
                    "text": string("What the samples say, 100–1,000 characters, in the voice's language and style (default: the provider writes one)"),
                    "name": string("What to call the voice")
                }),
                &["description", "name"],
            ),
        ),
        // ---- depth
        spec(
            "add_depth_effect",
            Some(Effects),
            "Propose a depth-aware effect on video or image clips from the pool, read from each frame's depth map (relative distance: 0 nearest, 1 farthest). lens_blur: blurs by distance, keeping `focus` sharp over `range`, `blur` in pixels (focus it on a point with set_depth_focus; keyframe `focus` with set_effect_keyframe for a rack focus). fog: haze of color `tint` thickening with `density` past distance `start`. camera: a camera move over the clip with parallax, the 3D Ken Burns look for stills and short clips: `motion` Dolly (in; out with negative `amount`), Zoom (dolly zoom about `focus`), Orbit or Pan (along `angle` degrees), `amount` -1..1. Returns each new effect's `@effect clip/effect` reference. For depth masks use set_mask's `depth` shape.",
            object(
                json!({
                    "clips": strings("Clip ids"),
                    "effect": one_of(&depth::EFFECTS.map(|(n, _)| n), "Which effect"),
                    "params": params("Parameter name → value; omitted ones keep their defaults"),
                    "reason": reason()
                }),
                &["clips", "effect"],
            ),
        ),
        spec(
            "set_depth_focus",
            Some(Effects),
            "Propose focusing a clip's lens_blur (or a camera move's pivot) on what is at `point` of the picture at `at`: the distance there becomes its `focus`. With `key`, or when `focus` is already keyframed, it is keyed at `at`; focus at two times for a rack focus. Waits for the frame's depth map.",
            object(
                json!({
                    "clip": string("Clip id"),
                    "effect": string("Effect id or `@effect clip/effect` (default: the clip's first lens_blur or camera)"),
                    "point": numbers(2, "[x, y], fractions of the picture from its top-left"),
                    "at": time("Timeline time of the frame (default: the clip's middle)"),
                    "key": boolean("Keyframe the focus at `at` (default: only when it already has keyframes)"),
                    "reason": reason()
                }),
                &["clip", "point"],
            ),
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
    ];
    tools.extend(super::stabilize::specs());
    // ---- tool groups (see `groups`)
    let index = groups::index(&tools);
    let keys: Vec<&str> = groups::GROUPS.iter().map(|g| groups::key(*g)).collect();
    tools.push(spec(
        groups::LOAD_TOOLS,
        None,
        "Only the tools most messages need are listed. Load one or more groups to get the schemas of their other tools, then call those through run_tool.",
        object(
            json!({ "groups": {
                "type": "array",
                "items": { "type": "string", "enum": keys },
                "description": format!("Groups to load. {index}"),
            } }),
            &["groups"],
        ),
    ));
    tools.push(spec(
        groups::RUN_TOOL,
        None,
        "Call a tool that load_tools handed you but that is not in your tool list yet.",
        object(
            json!({
                "name": string("The tool's name"),
                "arguments": { "type": "object", "description": "The tool's arguments, as its schema describes" },
            }),
            &["name"],
        ),
    ));
    tools
}
