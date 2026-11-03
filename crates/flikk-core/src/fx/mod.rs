//! Programmable effects: GLSL shaders for clip effects, transitions and
//! generators, and HTML/CSS/JS documents for motion graphics (titles, lower
//! thirds, callouts, animated text and images). One asset format for both,
//! shared by the inspector, the effects library, the agent tools, the viewer
//! and the render, so what the agent writes is what the user tweaks and what
//! the export shows.
//!
//! # Assets and instances
//!
//! An [`FxAsset`] is a program plus its parameter list. Built-ins ship with
//! the app ([`builtins`]) and are referenced by a stable key
//! (`FxRef::Builtin("gaussian_blur")`); project assets live in
//! [`Project::fx`] and are referenced by id. An [`FxInstance`] is one use of
//! an asset: on a clip's effect stack ([`Clip::effects`]), as a clip's
//! [`Transition`], or as the content of a generator clip
//! ([`ClipContent::Generator`]). It stores the parameter values that differ
//! from the asset's defaults, plus keyframes.
//!
//! # GLSL (effects, transitions, generators)
//!
//! Authors write one function; the engine adds `#version`, uniforms and
//! helpers. Coordinates follow ShaderToy and gl-transitions: `uv` is 0..1
//! with (0, 0) at the bottom-left. Colors are straight (not premultiplied)
//! RGBA in 0..1, in the frame's own sRGB encoding.
//!
//! Every program can use:
//! - `vec2 iResolution`: output size in pixels;
//! - `float iTime`: seconds since the clip (or transition) started;
//! - `float iDuration`: its length in seconds; `float iProgress` is
//!   `iTime / iDuration`;
//! - each parameter as a global of its name: Float → `float`; Int and
//!   Choice (option index) → `int`; Bool → `bool`; Color → `vec4`;
//!   Point → `vec2` (uv space); Image → `vec4 image_<name>(vec2 uv)`
//!   (transparent outside the picture). Text parameters are HTML-only.
//!
//! Per kind:
//! - Effect: `vec4 effect(vec2 uv)`; read the input with
//!   `vec4 getColor(vec2 uv)`.
//! - Transition (the gl-transitions convention): `vec4 transition(vec2 uv)`;
//!   read `getFromColor(uv)` and `getToColor(uv)`; `float progress` (0..1)
//!   and `float ratio` (width / height) are defined.
//! - Generator: `vec4 generate(vec2 uv)`; no input.
//!
//! # HTML (generators only: motion graphics, titles, image layouts)
//!
//! The code is an HTML document or fragment rendered with a transparent
//! background on a page of the sequence's shape with a 1080 CSS px short
//! side (1920 × 1080 for 16:9, 1080 × 1920 for 9:16, at any resolution),
//! then scaled to the output. The engine owns the clock, so frames are deterministic:
//! - GSAP timelines, CSS animations and transitions and the Web Animations
//!   API are paused and seeked to the clip time;
//! - `requestAnimationFrame`, `performance.now()`, `Date.now()` and timers
//!   follow the clip clock;
//! - `window.flikk = { time, duration, progress, width, height, params }` is
//!   current before each frame, and `window.flikkRender(time)` is called if
//!   the page defines it (for canvas or JS-driven drawing).
//!
//! Parameters reach the page three ways: `window.flikk.params.<name>`
//! (numbers, booleans and strings as they are; colors as `rgba()` strings;
//! points as `[x, y]` from the top-left; images as `file://` URLs); CSS
//! custom properties on `:root` (`--<name>`: numbers unitless, colors as
//! `rgba()`, points as `--<name>-x`/`--<name>-y` in 0..1 from the top-left,
//! images as `url(...)`); and `{{name}}` in the code, replaced by the value
//! before loading (text HTML-escaped, images as a `file://` URL usable in
//! `<img src>`). `:root` also gets `--duration`, `--width` and `--height`
//! (unitless), so CSS can schedule an exit animation with
//! `animation-delay: calc(var(--duration) * 1s - 0.4s)`.
//!
//! GSAP 3.15 is preloaded as `gsap`, with SplitText, CustomEase,
//! CustomBounce, CustomWiggle, EasePack, TextPlugin, ScrambleTextPlugin,
//! DrawSVGPlugin, MorphSVGPlugin, MotionPathPlugin, Physics2DPlugin and
//! PhysicsPropsPlugin registered. Build the animation in
//! `window.flikkTimeline = (flikk) => { ... }`: the engine runs it before the
//! first frame, and reverts and reruns it when parameters, duration or
//! canvas change and when seeking back, so it may read `flikk.params` and
//! `flikk.duration` (an exit placed at `flikk.duration - 0.5`). What it creates
//! starts at clip time 0; a function it returns runs before each rerun.
//! GSAP code outside it runs once, at load. Unit strings need a leading
//! digit (`'0.6em'`): GSAP reads `'.6em'` as pixels.

pub mod depth;
mod shaders;
mod templates;

use crate::ids::{EffectId, FxId, LibraryId};
use crate::model::{Clip, ClipContent, Ease, Project, Track};
use crate::time::{Time, TimeRange};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::OnceLock;

/// Which asset an instance uses.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FxRef {
    /// A shipped asset by its stable key, e.g. `cross_dissolve`.
    Builtin(String),
    /// An asset in [`Project::fx`].
    Project(FxId),
}

impl FxRef {
    pub fn builtin(key: &str) -> Self {
        Self::Builtin(key.to_owned())
    }
}

impl fmt::Display for FxRef {
    /// The key for built-ins, the short id for project assets: what the agent
    /// reads and passes back.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FxRef::Builtin(key) => f.write_str(key),
            FxRef::Project(id) => write!(f, "{id}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FxKind {
    /// Processes a clip's picture.
    Effect,
    /// Blends the outgoing and incoming pictures at a cut.
    Transition,
    /// Makes a picture from nothing (backgrounds, titles, motion graphics).
    Generator,
}

impl FxKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Effect => "Effect",
            Self::Transition => "Transition",
            Self::Generator => "Generator",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FxLanguage {
    Glsl,
    Html,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FxAsset {
    pub id: FxRef,
    pub name: String,
    pub kind: FxKind,
    pub language: FxLanguage,
    /// Library group, e.g. "Blur", "Stylize", "Lower thirds".
    #[serde(default)]
    pub category: String,
    /// One line for the library and the agent.
    #[serde(default)]
    pub description: String,
    /// GLSL function(s) or an HTML document; see the module docs.
    pub code: String,
    #[serde(default)]
    pub params: Vec<ParamSpec>,
    /// The library program a project program was copied from (see
    /// [`crate::library`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<LibraryId>,
}

impl FxAsset {
    pub fn param(&self, name: &str) -> Option<&ParamSpec> {
        self.params.iter().find(|p| p.name == name)
    }

    /// Structural problems the engines would reject anyway, in words the
    /// agent can act on. Compile errors come from the engine's `check`.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("the asset needs a name".into());
        }
        if self.language == FxLanguage::Html && self.kind != FxKind::Generator {
            return Err("only generators can be HTML; effects and transitions are GLSL".into());
        }
        let entry = match (self.language, self.kind) {
            (FxLanguage::Html, _) => None,
            (FxLanguage::Glsl, FxKind::Effect) => Some("effect"),
            (FxLanguage::Glsl, FxKind::Transition) => Some("transition"),
            (FxLanguage::Glsl, FxKind::Generator) => Some("generate"),
        };
        if let Some(entry) = entry
            && !self.code.contains(&format!("{entry}("))
        {
            return Err(format!("the GLSL code must define `vec4 {entry}(vec2 uv)`"));
        }
        let mut seen = Vec::new();
        for p in &self.params {
            let ident = !p.name.is_empty()
                && p.name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !p.name.starts_with(|c: char| c.is_ascii_digit());
            if !ident {
                return Err(format!(
                    "parameter `{}` must be an identifier (letters, digits, _)",
                    p.name
                ));
            }
            if seen.contains(&&p.name) {
                return Err(format!("parameter `{}` is declared twice", p.name));
            }
            seen.push(&p.name);
            if self.language == FxLanguage::Glsl && matches!(p.kind, ParamKind::Text) {
                return Err(format!(
                    "parameter `{}`: text parameters are HTML-only",
                    p.name
                ));
            }
            if !p.kind.accepts(&p.default) {
                return Err(format!(
                    "parameter `{}`: default does not fit its type",
                    p.name
                ));
            }
        }
        Ok(())
    }
}

/// One parameter of an asset: what the inspector shows a control for and
/// what the agent may set.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParamSpec {
    /// Identifier used in code (uniform, CSS variable, `{{name}}`).
    pub name: String,
    #[serde(default)]
    pub label: String,
    #[serde(flatten)]
    pub kind: ParamKind,
    pub default: ParamValue,
}

impl ParamSpec {
    pub fn label(&self) -> &str {
        if self.label.is_empty() {
            &self.name
        } else {
            &self.label
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ParamKind {
    Float {
        min: f64,
        max: f64,
    },
    Int {
        min: i64,
        max: i64,
    },
    Bool,
    /// `[r, g, b, a]`, 0..1.
    Color,
    /// `[x, y]` in uv space (0..1, origin bottom-left for GLSL; HTML gets
    /// it flipped to top-left).
    Point,
    /// HTML only; single or multi-line.
    Text,
    /// Stored as the option's text; GLSL receives its index.
    Choice {
        options: Vec<String>,
    },
    /// Path of an image file (PNG, JPEG, WebP, SVG for HTML).
    Image,
}

impl ParamKind {
    /// Whether `value` has the right shape for this kind.
    pub fn accepts(&self, value: &ParamValue) -> bool {
        match (self, value) {
            (ParamKind::Float { .. } | ParamKind::Int { .. }, ParamValue::Number(_)) => true,
            (ParamKind::Bool, ParamValue::Bool(_)) => true,
            (ParamKind::Color, ParamValue::List(v)) => v.len() == 4,
            (ParamKind::Point, ParamValue::List(v)) => v.len() == 2,
            (ParamKind::Text | ParamKind::Image, ParamValue::Text(_)) => true,
            (ParamKind::Choice { options }, ParamValue::Text(t)) => options.contains(t),
            _ => false,
        }
    }

    /// `value` made valid for this kind: numbers clamped, wrong shapes
    /// replaced by `fallback`.
    pub fn coerce(&self, value: &ParamValue, fallback: &ParamValue) -> ParamValue {
        match (self, value) {
            (ParamKind::Float { min, max }, ParamValue::Number(v)) => {
                ParamValue::Number(v.clamp(*min, max.max(*min)))
            }
            (ParamKind::Int { min, max }, ParamValue::Number(v)) => {
                ParamValue::Number(v.round().clamp(*min as f64, (*max).max(*min) as f64))
            }
            (ParamKind::Color, ParamValue::List(v)) if v.len() == 4 => {
                ParamValue::List(v.iter().map(|c| c.clamp(0.0, 1.0)).collect())
            }
            (ParamKind::Color, ParamValue::List(v)) if v.len() == 3 => {
                let mut c: Vec<f64> = v.iter().map(|c| c.clamp(0.0, 1.0)).collect();
                c.push(1.0);
                ParamValue::List(c)
            }
            _ if self.accepts(value) => value.clone(),
            _ => fallback.clone(),
        }
    }
}

/// A parameter value. Untagged in JSON: `true`, `0.5`, `[1, 0, 0, 1]`, `"Hi"`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParamValue {
    Bool(bool),
    Number(f64),
    /// Colors and points.
    List(Vec<f64>),
    /// Text, choice options and image paths.
    Text(String),
}

impl ParamValue {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            ParamValue::Number(v) => Some(*v),
            ParamValue::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[f64]> {
        match self {
            ParamValue::List(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            ParamValue::Bool(b) => Some(*b),
            ParamValue::Number(v) => Some(*v != 0.0),
            _ => None,
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            ParamValue::Text(t) => Some(t),
            _ => None,
        }
    }

    /// Eased blend from `self` to `to` at `u` (0..1). Numbers and lists of
    /// equal length interpolate; anything else holds `self` until `u` = 1.
    pub fn lerp(&self, to: &ParamValue, u: f64) -> ParamValue {
        match (self, to) {
            (ParamValue::Number(a), ParamValue::Number(b)) => ParamValue::Number(a + (b - a) * u),
            (ParamValue::List(a), ParamValue::List(b)) if a.len() == b.len() => {
                ParamValue::List(a.iter().zip(b).map(|(a, b)| a + (b - a) * u).collect())
            }
            _ if u >= 1.0 => to.clone(),
            _ => self.clone(),
        }
    }
}

fn yes() -> bool {
    true
}

/// One use of an asset.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FxInstance {
    #[serde(default)]
    pub id: EffectId,
    pub fx: FxRef,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Values that differ from the asset's defaults.
    #[serde(default)]
    pub params: BTreeMap<String, ParamValue>,
    /// Animated parameters. Times are relative to the clip's start (or the
    /// transition's start) and may lie outside it after trims, so a curve
    /// keeps its shape when the clip is cut.
    #[serde(default)]
    pub keyframes: Vec<ParamKeyframe>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParamKeyframe {
    pub param: String,
    pub at: Time,
    pub value: ParamValue,
    /// Easing into the next keyframe of the same parameter.
    #[serde(default)]
    pub ease: Ease,
}

impl FxInstance {
    pub fn new(fx: FxRef) -> Self {
        Self {
            id: EffectId::new(),
            fx,
            enabled: true,
            params: BTreeMap::new(),
            keyframes: Vec::new(),
        }
    }

    /// Every parameter's value at `t` (relative to the clip or transition
    /// start): the asset's defaults, overridden by `params`, overridden by
    /// keyframes (held before the first and after the last, eased between).
    /// Values are coerced to their parameter's kind.
    pub fn values_at(&self, asset: &FxAsset, t: Time) -> BTreeMap<String, ParamValue> {
        asset
            .params
            .iter()
            .map(|spec| {
                let base = self.params.get(&spec.name).unwrap_or(&spec.default);
                let value = self.keyed(&spec.name, t).unwrap_or_else(|| base.clone());
                (spec.name.clone(), spec.kind.coerce(&value, &spec.default))
            })
            .collect()
    }

    /// The keyframed value of `param` at `t`, if it has keyframes.
    pub fn keyed(&self, param: &str, t: Time) -> Option<ParamValue> {
        let (mut prev, mut next): (Option<&ParamKeyframe>, Option<&ParamKeyframe>) = (None, None);
        for k in self.keyframes.iter().filter(|k| k.param == param) {
            if k.at <= t {
                if prev.is_none_or(|p| k.at >= p.at) {
                    prev = Some(k);
                }
            } else if next.is_none_or(|n| k.at < n.at) {
                next = Some(k);
            }
        }
        match (prev, next) {
            (None, None) => None,
            (Some(k), None) | (None, Some(k)) => Some(k.value.clone()),
            (Some(a), Some(b)) => {
                let u = (t - a.at).0 as f64 / (b.at - a.at).0 as f64;
                Some(a.value.lerp(&b.value, a.ease.apply(u as f32) as f64))
            }
        }
    }

    /// Shift every keyframe by `by` (trims move a clip's origin).
    pub fn shift_keyframes(&mut self, by: Time) {
        for k in &mut self.keyframes {
            k.at = k.at + by;
        }
    }

    /// Stretch keyframe times by `factor` (speed changes).
    pub fn scale_keyframes(&mut self, factor: f64) {
        for k in &mut self.keyframes {
            k.at = k.at.scale(factor);
        }
    }
}

/// Where a transition sits relative to its cut.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionAlign {
    /// Half before the cut, half after (Resolve's "Center on Edit").
    #[default]
    Center,
    /// Starts at the cut.
    StartOnCut,
    /// Ends at the cut.
    EndOnCut,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transition {
    /// A [`FxKind::Transition`] asset. Its clock (`iTime`, keyframes) starts
    /// at the transition's start.
    pub fx: FxInstance,
    pub duration: Time,
    #[serde(default)]
    pub align: TransitionAlign,
}

impl Transition {
    pub fn new(fx: FxRef, duration: Time) -> Self {
        Self {
            fx: FxInstance::new(fx),
            duration,
            align: TransitionAlign::Center,
        }
    }
}

/// A transition placed on the timeline.
#[derive(Clone, Debug)]
pub struct PlacedTransition<'a> {
    pub transition: &'a Transition,
    /// The cut, or the clip edge when one side is empty.
    pub at: Time,
    /// Timeline time the blend covers; progress is 0 at its start.
    pub range: TimeRange,
    /// Outgoing picture: `None` = from nothing (transparent).
    pub from: Option<&'a Clip>,
    /// Incoming picture: `None` = to nothing (transparent).
    pub to: Option<&'a Clip>,
}

impl PlacedTransition<'_> {
    /// 0..1 at timeline time `t`.
    pub fn progress(&self, t: Time) -> f32 {
        let d = self.range.duration().0.max(1) as f64;
        ((t - self.range.start).0 as f64 / d).clamp(0.0, 1.0) as f32
    }
}

/// Every transition on `track`, resolved against its neighbours.
///
/// At a cut between adjacent clips A → B the incoming clip's
/// `transition_in` wins, else the outgoing clip's `transition_out`; the
/// blend covers its range around the cut per [`TransitionAlign`]. During it
/// A plays past its end and B before its start from their source handles
/// (renderers hold the edge frame when a source runs out). At an edge with
/// no neighbour the transition always lies inside the clip: a head
/// transition fades in from nothing, a tail transition fades out to nothing.
/// Disabled clips count as nothing.
pub fn transitions(track: &Track) -> Vec<PlacedTransition<'_>> {
    let clips: Vec<&Clip> = track.clips.iter().filter(|c| c.enabled).collect();
    let mut out = Vec::new();
    for (i, clip) in clips.iter().enumerate() {
        let prev = i
            .checked_sub(1)
            .map(|p| clips[p])
            .filter(|p| p.end() == clip.start);
        let next = clips.get(i + 1).copied().filter(|n| n.start == clip.end());
        // The cut before this clip.
        let at_head = match (&clip.transition_in, prev) {
            (Some(t), _) => Some(t),
            (None, Some(p)) => p.transition_out.as_ref(),
            (None, None) => None,
        };
        if let Some(t) = at_head {
            let at = clip.start;
            let d = t.duration.max(Time::ZERO);
            let range = match prev {
                None => TimeRange::new(at, at + d.min(clip.duration)),
                Some(_) => match t.align {
                    TransitionAlign::Center => {
                        let half = Time(d.0 / 2);
                        TimeRange::new(at - half, at - half + d)
                    }
                    TransitionAlign::StartOnCut => TimeRange::new(at, at + d),
                    TransitionAlign::EndOnCut => TimeRange::new(at - d, at),
                },
            };
            if !range.is_empty() {
                out.push(PlacedTransition {
                    transition: t,
                    at,
                    range,
                    from: prev,
                    to: Some(clip),
                });
            }
        }
        // A tail with nothing after it.
        if next.is_none()
            && let Some(t) = &clip.transition_out
        {
            let at = clip.end();
            let d = t.duration.max(Time::ZERO).min(clip.duration);
            let range = TimeRange::new(at - d, at);
            if !range.is_empty() {
                out.push(PlacedTransition {
                    transition: t,
                    at,
                    range,
                    from: Some(clip),
                    to: None,
                });
            }
        }
    }
    out
}

/// Shipped assets: shaders (effects, transitions, generators) and HTML
/// templates.
pub fn builtins() -> &'static [FxAsset] {
    static ALL: OnceLock<Vec<FxAsset>> = OnceLock::new();
    ALL.get_or_init(|| {
        let mut all = shaders::all();
        all.extend(templates::all());
        all.extend(depth::all());
        all
    })
}

/// The asset `r` names: a built-in or one of the project's.
pub fn asset<'a>(project: &'a Project, r: &FxRef) -> Option<&'a FxAsset> {
    match r {
        FxRef::Builtin(_) => builtins().iter().find(|a| &a.id == r),
        FxRef::Project(_) => project.fx.iter().find(|a| &a.id == r),
    }
}

/// Built-ins first, then the project's assets.
pub fn library(project: &Project) -> impl Iterator<Item = &FxAsset> {
    builtins().iter().chain(project.fx.iter())
}

/// Resolve what a user or the agent typed: a built-in key, a project asset's
/// (short) id, or an asset name (case-insensitive).
pub fn find(project: &Project, text: &str) -> Option<FxRef> {
    let text = text.trim();
    let lower = text.to_lowercase();
    if let Some(a) = builtins()
        .iter()
        .find(|a| matches!(&a.id, FxRef::Builtin(k) if *k == lower))
    {
        return Some(a.id.clone());
    }
    let hex = lower.replace('-', "");
    if hex.len() >= 4
        && hex.chars().all(|c| c.is_ascii_hexdigit())
        && let Some(a) = project.fx.iter().find(
            |a| matches!(&a.id, FxRef::Project(id) if id.0.simple().to_string().starts_with(&hex)),
        )
    {
        return Some(a.id.clone());
    }
    library(project)
        .find(|a| a.name.to_lowercase() == lower)
        .map(|a| a.id.clone())
}

/// Every place `r` is used in the project, as clip ids.
pub fn uses(project: &Project, r: &FxRef) -> Vec<crate::ids::ClipId> {
    project
        .timeline
        .tracks
        .iter()
        .flat_map(|t| &t.clips)
        .filter(|c| {
            c.effects.iter().any(|e| &e.fx == r)
                || [&c.transition_in, &c.transition_out]
                    .iter()
                    .any(|t| t.as_ref().is_some_and(|t| &t.fx.fx == r))
                || matches!(&c.content, ClipContent::Generator { fx } if &fx.fx == r)
        })
        .map(|c| c.id)
        .collect()
}

// Shared by the built-in tables.
fn float(name: &str, label: &str, min: f64, max: f64, default: f64) -> ParamSpec {
    ParamSpec {
        name: name.into(),
        label: label.into(),
        kind: ParamKind::Float { min, max },
        default: ParamValue::Number(default),
    }
}

fn color(name: &str, label: &str, rgba: [f64; 4]) -> ParamSpec {
    ParamSpec {
        name: name.into(),
        label: label.into(),
        kind: ParamKind::Color,
        default: ParamValue::List(rgba.to_vec()),
    }
}

fn text(name: &str, label: &str, default: &str) -> ParamSpec {
    ParamSpec {
        name: name.into(),
        label: label.into(),
        kind: ParamKind::Text,
        default: ParamValue::Text(default.into()),
    }
}
