//! The style profile's caption styles: reading them, and asking to create,
//! edit or delete one. The styles live outside the project, so a change is
//! an [`Op`] the UI carries out once the user approves it (or at once, if
//! they allow it).

use super::{Done, Memory, ToolCtx};
use crate::args::{Args, ToolError};
use flikk_core::memory::{CaptionStyle, CaptionStylePatch, Color, MotionPatch};
use flikk_core::model::{Project, Track, TrackKind};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// The tools that change caption styles.
pub const TOOLS: [&str; 3] = [
    "create_caption_style",
    "edit_caption_style",
    "delete_caption_style",
];

/// A change to the profile's caption styles, as asked and approved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// Add `style`, ready made.
    Create { style: CaptionStyle },
    /// Change the style named `style` as it is when approved: dressed in
    /// `look` (as clicking a look does), then changed by `patch`, then
    /// renamed to `rename`.
    Edit {
        style: String,
        look: Option<CaptionStyle>,
        patch: Box<CaptionStylePatch>,
        rename: Option<String>,
    },
    /// Remove the style named `style`; tracks styled with it fall back to
    /// the first.
    Delete { style: String },
}

impl Op {
    /// The op an approval request carries, as [`request`] wrote it.
    pub fn from_json(args: &Value) -> Option<Op> {
        serde_json::from_value(args.clone()).ok()
    }
}

/// The style a caption track is lettered in: the one it names, else the
/// profile's first, else the default.
pub(crate) fn of_track(memory: &Memory, track: Option<&Track>) -> CaptionStyle {
    let styles = &memory.caption_styles;
    track
        .and_then(|t| t.caption_style.as_deref())
        .and_then(|name| styles.iter().find(|s| s.name == name))
        .or(styles.first())
        .cloned()
        .unwrap_or_default()
}

/// The style named `name` (its case aside).
pub(crate) fn find<'a>(memory: &'a Memory, name: &str) -> Result<&'a CaptionStyle, ToolError> {
    let styles = &memory.caption_styles;
    let name = name.trim();
    styles
        .iter()
        .find(|s| s.name == name)
        .or_else(|| styles.iter().find(|s| s.name.eq_ignore_ascii_case(name)))
        .ok_or_else(|| {
            let names: Vec<_> = styles.iter().map(|s| format!("“{}”", s.name)).collect();
            ToolError(format!(
                "No caption style “{name}”. The styles are {}.",
                names.join(", ")
            ))
        })
}

/// `list_caption_styles`: every style in full, the tracks lettered in
/// each, the one new captions use and the ready-made looks.
pub(crate) fn list(ctx: &ToolCtx) -> Result<Done, ToolError> {
    let project = ctx.project();
    let memory = ctx.memory;
    let new_captions = of_track(memory, first_caption_track(project)).name;
    let styles: Vec<Value> = memory
        .caption_styles
        .iter()
        .map(|style| {
            let mut v = serde_json::to_value(style).unwrap_or_default();
            let tracks: Vec<Value> = caption_tracks(project)
                .filter(|t| of_track(memory, Some(t)).name == style.name)
                .map(|t| json!({ "id": t.id.0.to_string(), "name": t.name }))
                .collect();
            v["tracks"] = json!(tracks);
            v
        })
        .collect();
    let looks: Vec<String> = CaptionStyle::looks().into_iter().map(|l| l.name).collect();
    let value = json!({
        "styles": styles,
        "new_captions_use": new_captions,
        "looks": looks,
        "units": "size, spacing, outline, shadow and padding in px at 1080p (they scale with the frame); offset_x/offset_y in % of the frame, right and up; shadow_angle in degrees clockwise from the right; durations in seconds; colors #RRGGBBAA",
    });
    Ok(Done::json(
        format!("{} caption styles", memory.caption_styles.len()),
        &value,
    ))
}

/// Validate a call of one of [`TOOLS`] against the styles there are: the
/// line for the approval card and the [`Op`] it carries.
pub(crate) fn request(
    memory: &Memory,
    tool: &str,
    args: &Args,
) -> Result<(String, Value), ToolError> {
    let (summary, op) = match tool {
        "create_caption_style" => {
            let name = free_name(memory, args.str("name")?, None)?;
            let from = args.opt_str("from")?.map(|f| base(memory, f)).transpose()?;
            let mut summary = format!("New caption style “{name}”");
            if let Some(from) = &from {
                summary.push_str(&format!(" from {}", from.name));
            }
            let mut style = from.unwrap_or_default();
            let patch = patch(args, &["name", "from"])?;
            style.name = name;
            patch.apply(&mut style);
            (
                with_changes(summary, &describe(&patch)),
                Op::Create { style },
            )
        }
        "edit_caption_style" => {
            let style = find(memory, args.str("style")?)?.name.clone();
            let rename = match args.opt_str("name")? {
                Some(name) if name.trim() != style => Some(free_name(memory, name, Some(&style))?),
                _ => None,
            };
            let look = match args.opt_str("look")? {
                Some(name) => Some(CaptionStyle::look(name).ok_or_else(|| {
                    ToolError(format!("No look “{name}”. The looks are {}.", look_names()))
                })?),
                None => None,
            };
            let patch = patch(args, &["style", "name", "look"])?;
            if look.is_none() && patch.is_empty() && rename.is_none() {
                return Err(ToolError(
                    "Nothing to change: give a `look`, a new `name` or parameters.".into(),
                ));
            }
            let mut changes = Vec::new();
            if let Some(look) = &look {
                changes.push(format!("{} look", look.name));
            }
            changes.extend(describe(&patch));
            if let Some(name) = &rename {
                changes.push(format!("renamed “{name}”"));
            }
            (
                with_changes(format!("Caption style “{style}”"), &changes),
                Op::Edit {
                    style,
                    look,
                    patch: Box::new(patch),
                    rename,
                },
            )
        }
        "delete_caption_style" => {
            let style = find(memory, args.str("style")?)?.name.clone();
            if memory.caption_styles.len() < 2 {
                return Err(ToolError(
                    "The last caption style cannot be deleted.".into(),
                ));
            }
            (
                format!("Delete caption style “{style}”"),
                Op::Delete { style },
            )
        }
        other => return Err(ToolError(format!("`{other}` is not a caption style tool"))),
    };
    let value = serde_json::to_value(&op).map_err(|e| ToolError(e.to_string()))?;
    Ok((summary, value))
}

/// What a new style starts from: a style of the profile, else a look.
fn base(memory: &Memory, name: &str) -> Result<CaptionStyle, ToolError> {
    if let Ok(style) = find(memory, name) {
        return Ok(style.clone());
    }
    CaptionStyle::look(name).ok_or_else(|| {
        let styles: Vec<_> = memory
            .caption_styles
            .iter()
            .map(|s| format!("“{}”", s.name))
            .collect();
        ToolError(format!(
            "No caption style or look “{name}”. The styles are {}; the looks are {}.",
            styles.join(", "),
            look_names()
        ))
    })
}

fn look_names() -> String {
    let names: Vec<_> = CaptionStyle::looks()
        .into_iter()
        .map(|l| format!("“{}”", l.name))
        .collect();
    names.join(", ")
}

/// `name`, trimmed, when no style but `renaming` has it.
fn free_name(memory: &Memory, name: &str, renaming: Option<&str>) -> Result<String, ToolError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(ToolError("A caption style needs a name.".into()));
    }
    let taken = memory
        .caption_styles
        .iter()
        .any(|s| s.name.eq_ignore_ascii_case(name) && Some(s.name.as_str()) != renaming);
    if taken {
        return Err(ToolError(format!(
            "There is a caption style “{name}” already; pick another name or edit that one."
        )));
    }
    Ok(name.to_owned())
}

/// The style parameters among `args`, leaving out the tool's own `keys`
/// (and a `reason`, which these tools don't take but models send).
fn patch(args: &Args, keys: &[&str]) -> Result<CaptionStylePatch, ToolError> {
    let params: Map<String, Value> = args
        .0
        .iter()
        .filter(|(k, _)| k.as_str() != "reason" && !keys.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    serde_json::from_value(Value::Object(params))
        .map_err(|e| ToolError(format!("Style parameters: {e}")))
}

fn with_changes(summary: String, changes: &[String]) -> String {
    if changes.is_empty() {
        summary
    } else {
        format!("{summary}: {}", changes.join(", "))
    }
}

/// The parameters `patch` sets, in a few words each ("size 60", "fill
/// #FFE14D", "intro pop 0.15 s").
fn describe(patch: &CaptionStylePatch) -> Vec<String> {
    let mut out = Vec::new();
    let mut num = |label: &str, v: Option<f32>, unit: &str| {
        if let Some(v) = v {
            out.push(format!("{label} {}{unit}", round(v)));
        }
    };
    num("size", patch.size, "");
    num("spacing", patch.spacing, "");
    num("outline", patch.outline, "");
    num("shadow", patch.shadow, "");
    num("shadow angle", patch.shadow_angle, "°");
    num("x", patch.offset_x, "%");
    num("y", patch.offset_y, "%");
    num("line length", patch.max_chars_per_line, " ch");
    num("lines", patch.max_lines, "");
    if let Some(font) = &patch.font {
        out.push(format!("font {font}"));
    }
    for (label, on) in [
        ("bold", patch.bold),
        ("italic", patch.italic),
        ("uppercase", patch.uppercase),
    ] {
        match on {
            Some(true) => out.push(label.to_owned()),
            Some(false) => out.push(format!("not {label}")),
            None => {}
        }
    }
    for (label, color) in [
        ("fill", patch.color),
        ("outline color", patch.outline_color),
        ("shadow color", patch.shadow_color),
        ("highlight color", patch.highlight_color),
    ] {
        if let Some(c) = color {
            out.push(format!("{label} {}", hex(c)));
        }
    }
    if let Some(b) = &patch.background {
        match b.enabled {
            Some(true) => out.push("background on".into()),
            Some(false) => out.push("background off".into()),
            None => {}
        }
        if let Some(c) = b.color {
            out.push(format!("background {}", hex(c)));
        }
        if let Some(p) = b.padding {
            out.push(format!("padding {}", round(p)));
        }
    }
    if let Some(p) = patch.position {
        out.push(format!("{} position", to_word(&p)));
    }
    if let Some(h) = patch.highlight {
        out.push(format!("{} highlight", to_word(&h)));
    }
    for (label, motion) in [("intro", &patch.intro), ("outro", &patch.outro)] {
        if let Some(MotionPatch { kind, duration }) = motion {
            let mut words = vec![label.to_owned()];
            words.extend(kind.map(|k| to_word(&k)));
            words.extend(duration.map(|d| format!("{} s", round(d))));
            out.push(words.join(" "));
        }
    }
    out
}

/// A serde enum as its word ("bottom", "karaoke").
fn to_word(v: &impl Serialize) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn round(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

/// `#RRGGBB`, with the alpha only when it is not opaque.
fn hex(c: Color) -> String {
    let s = String::from(c);
    match s.strip_suffix("FF") {
        Some(opaque) if c.a == 255 => opaque.to_owned(),
        _ => s,
    }
}

fn caption_tracks(project: &Project) -> impl Iterator<Item = &Track> {
    project
        .timeline
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Caption)
}

fn first_caption_track(project: &Project) -> Option<&Track> {
    caption_tracks(project).next()
}
