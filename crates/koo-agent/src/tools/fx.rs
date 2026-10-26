//! Effects, transitions and generators on clips, and the project's own FX
//! programs: arguments → `Command`s proposed through the bus. Authoring
//! (`create_fx`, `update_fx`) validates and compiles before proposing.

use super::edit::{ease, edge, propose_then, track_or_new};
use super::{Done, Step, ToolCtx, propose};
use crate::args::*;
use koo_core::command::{Command, Edge};
use koo_core::fx::{
    self, FxAsset, FxInstance, FxKind, FxLanguage, FxRef, ParamKind, ParamSpec, ParamValue,
    Transition, TransitionAlign,
};
use koo_core::model::{Clip, ClipContent, Project, TrackKind};
use koo_core::services::FxCall;
use koo_core::{EffectId, FxId, Time};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Edit tools served here.
pub const TOOLS: [&str; 10] = [
    "add_effect",
    "set_effect",
    "remove_effect",
    "move_effect",
    "set_effect_keyframe",
    "remove_effect_keyframe",
    "set_transition",
    "remove_transition",
    "add_generator",
    "set_generator",
];

/// Default transition length when none is given or set.
const TRANSITION: f64 = 0.5;

pub fn kind_name(kind: FxKind) -> &'static str {
    match kind {
        FxKind::Effect => "effect",
        FxKind::Transition => "transition",
        FxKind::Generator => "generator",
    }
}

pub fn language_name(language: FxLanguage) -> &'static str {
    match language {
        FxLanguage::Glsl => "glsl",
        FxLanguage::Html => "html",
    }
}

/// The asset `text` names (`@fx` token, built-in key, project id or name),
/// checked against `kind`.
pub fn fx_asset<'a>(
    project: &'a Project,
    text: &str,
    kind: Option<FxKind>,
) -> ToolResultOf<&'a FxAsset> {
    let text = strip_token(text);
    let asset = fx::find(project, text)
        .and_then(|r| fx::asset(project, &r))
        .ok_or_else(|| {
            ToolError(format!(
                "no FX program `{text}`; list_fx shows the built-in keys and project ids"
            ))
        })?;
    match kind {
        Some(k) if asset.kind != k => fail(format!(
            "`{}` is a {}, not a {}",
            asset.id,
            kind_name(asset.kind),
            kind_name(k)
        )),
        _ => Ok(asset),
    }
}

/// One parameter value from JSON, checked against its spec. Choices also
/// take the option index; colors take `[r, g, b]` or `"#rrggbb"`.
pub fn param_value(spec: &ParamSpec, v: &Value) -> ToolResultOf<ParamValue> {
    let value = match (&spec.kind, v) {
        (ParamKind::Choice { options }, Value::Number(n)) => n
            .as_u64()
            .and_then(|i| options.get(i as usize))
            .map(|o| ParamValue::Text(o.clone())),
        (ParamKind::Color, Value::String(hex)) => hex_color(hex).map(ParamValue::List),
        _ => serde_json::from_value::<ParamValue>(v.clone()).ok(),
    };
    let shape = || {
        ToolError(format!(
            "`{}` takes {}",
            spec.name,
            match &spec.kind {
                ParamKind::Float { min, max } => format!("a number {min}..{max}"),
                ParamKind::Int { min, max } => format!("an integer {min}..{max}"),
                ParamKind::Bool => "true or false".into(),
                ParamKind::Color => "[r, g, b, a] in 0..1 or \"#rrggbb\"".into(),
                ParamKind::Point => "[x, y] in 0..1".into(),
                ParamKind::Text => "text".into(),
                ParamKind::Choice { options } => format!("one of {}", options.join(", ")),
                ParamKind::Image => "an image file path".into(),
            }
        ))
    };
    let value = value.ok_or_else(shape)?;
    let coerced = spec.kind.coerce(&value, &spec.default);
    if !spec.kind.accepts(&value) && coerced == spec.default && value != spec.default {
        return Err(shape());
    }
    Ok(coerced)
}

fn hex_color(s: &str) -> Option<Vec<f64>> {
    let h = s.trim().trim_start_matches('#');
    if !(h.len() == 6 || h.len() == 8) || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let mut out: Vec<f64> = (0..h.len() / 2)
        .map(|i| u8::from_str_radix(&h[i * 2..i * 2 + 2], 16).unwrap_or(0) as f64 / 255.0)
        .collect();
    if out.len() == 3 {
        out.push(1.0);
    }
    Some(out)
}

/// `params` (an object of name → value) checked against `asset`.
pub fn params(asset: &FxAsset, v: Option<&Value>) -> ToolResultOf<BTreeMap<String, ParamValue>> {
    let Some(v) = v else {
        return Ok(BTreeMap::new());
    };
    let map = v
        .as_object()
        .ok_or_else(|| ToolError("`params` is an object of name → value".into()))?;
    map.iter()
        .map(|(name, value)| {
            let spec = asset.param(name).ok_or_else(|| {
                let names: Vec<_> = asset.params.iter().map(|p| p.name.as_str()).collect();
                ToolError(format!(
                    "`{}` has no parameter `{name}`; it has {}",
                    asset.name,
                    if names.is_empty() {
                        "none".to_owned()
                    } else {
                        names.join(", ")
                    }
                ))
            })?;
            Ok((name.clone(), param_value(spec, value)?))
        })
        .collect()
}

/// Where an FX instance sits on a clip.
#[derive(Clone, Copy, PartialEq)]
enum Slot {
    Effect,
    Generator,
    Transition(Edge),
}

/// The instance `text` names on `clip`: an effect id (or `@effect c/e`),
/// `in`/`out` (or `@transition c/in`) for transitions, or `generator`.
fn instance<'a>(clip: &'a Clip, text: &str) -> ToolResultOf<(Slot, &'a FxInstance)> {
    let t = strip_token(text);
    let t = t
        .rsplit('/')
        .next()
        .unwrap_or(t)
        .trim()
        .to_ascii_lowercase();
    let by_slot = match t.as_str() {
        "in" | "start" => clip
            .transition_in
            .as_ref()
            .map(|x| (Slot::Transition(Edge::Start), &x.fx)),
        "out" | "end" => clip
            .transition_out
            .as_ref()
            .map(|x| (Slot::Transition(Edge::End), &x.fx)),
        "generator" => match &clip.content {
            ClipContent::Generator { fx } => Some((Slot::Generator, fx)),
            _ => None,
        },
        _ => None,
    };
    if let Some(found) = by_slot {
        return Ok(found);
    }
    let matches = |id: &EffectId| t.len() >= 4 && id.0.simple().to_string().starts_with(&t);
    let mut all: Vec<(Slot, &FxInstance)> =
        clip.effects.iter().map(|e| (Slot::Effect, e)).collect();
    if let ClipContent::Generator { fx } = &clip.content {
        all.push((Slot::Generator, fx));
    }
    if let Some(x) = &clip.transition_in {
        all.push((Slot::Transition(Edge::Start), &x.fx));
    }
    if let Some(x) = &clip.transition_out {
        all.push((Slot::Transition(Edge::End), &x.fx));
    }
    let found: Vec<_> = all.into_iter().filter(|(_, e)| matches(&e.id)).collect();
    match found.as_slice() {
        [one] => Ok(*one),
        [] => fail(format!("{} has no effect `{text}`", clip.id)),
        _ => fail(format!("`{text}` matches several effects; use more digits")),
    }
}

fn clip_of<'a>(project: &'a Project, args: &Args) -> ToolResultOf<&'a Clip> {
    let id = clip_id(project, args.str("clip")?)?;
    project
        .timeline
        .clip(id)
        .map(|(_, c)| c)
        .ok_or_else(|| ToolError("clip not found".into()))
}

fn align(args: &Args) -> ToolResultOf<Option<TransitionAlign>> {
    Ok(match args.opt_str("align")? {
        None => None,
        Some("center") => Some(TransitionAlign::Center),
        Some("start_on_cut") => Some(TransitionAlign::StartOnCut),
        Some("end_on_cut") => Some(TransitionAlign::EndOnCut),
        Some(a) => return fail(format!("unknown align `{a}`")),
    })
}

/// The new instance's params merged over `base`.
fn merged(
    base: &FxInstance,
    asset: &FxAsset,
    args: &Args,
) -> ToolResultOf<BTreeMap<String, ParamValue>> {
    let mut out = if args.opt_bool("reset_params")? == Some(true) {
        BTreeMap::new()
    } else {
        base.params.clone()
    };
    out.extend(params(asset, args.value("params"))?);
    Ok(out)
}

pub fn serve(ctx: &mut ToolCtx, tool: &str, args: &Args) -> Result<Step, ToolError> {
    let reason = args
        .opt_str("reason")?
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .unwrap_or(tool)
        .to_owned();
    let p = ctx.project();
    let mut note = String::new();
    let commands = match tool {
        "add_effect" => {
            let asset = fx_asset(p, args.str("fx")?, Some(FxKind::Effect))?;
            let clips = clip_ids(p, &args.strings("clips")?)?;
            if clips.is_empty() {
                return fail("`clips` is empty");
            }
            let mut effect = FxInstance::new(asset.id.clone());
            effect.params = params(asset, args.value("params"))?;
            if let Some(e) = args.opt_bool("enabled")? {
                effect.enabled = e;
            }
            let before: Vec<Vec<EffectId>> = clips
                .iter()
                .map(|id| {
                    p.timeline
                        .clip(*id)
                        .map(|(_, c)| c.effects.iter().map(|e| e.id).collect())
                        .unwrap_or_default()
                })
                .collect();
            let command = Command::AddEffect {
                clips: clips.clone(),
                effect,
                index: args.opt_f64("index")?.map(|i| i.max(0.0) as usize),
            };
            let mut done = propose(ctx, vec![command], &reason);
            if !done.result.is_error {
                let added: Vec<String> = clips
                    .iter()
                    .zip(&before)
                    .filter_map(|(id, before)| {
                        let (_, c) = ctx.project().timeline.clip(*id)?;
                        let e = c.effects.iter().find(|e| !before.contains(&e.id))?;
                        Some(format!("@effect {id}/{}", e.id))
                    })
                    .collect();
                append(&mut done, &format!("New effects: {}", added.join(", ")));
            }
            return Ok(done.into());
        }
        "set_effect" => {
            let clip = clip_of(p, args)?;
            let (slot, inst) = instance(clip, args.str("effect")?)?;
            if slot != Slot::Effect {
                return fail("that is not a clip effect; use set_transition or set_generator");
            }
            let asset = fx::asset(p, &inst.fx)
                .ok_or_else(|| ToolError(format!("no FX program `{}`", inst.fx)))?;
            let mut effect = inst.clone();
            effect.params = merged(inst, asset, args)?;
            if let Some(e) = args.opt_bool("enabled")? {
                effect.enabled = e;
            }
            vec![Command::SetEffect {
                clip: clip.id,
                effect,
            }]
        }
        "remove_effect" | "move_effect" => {
            let clip = clip_of(p, args)?;
            let (slot, inst) = instance(clip, args.str("effect")?)?;
            if slot != Slot::Effect {
                return fail("that is not a clip effect");
            }
            if tool == "remove_effect" {
                vec![Command::RemoveEffect {
                    clip: clip.id,
                    effect: inst.id,
                }]
            } else {
                vec![Command::MoveEffect {
                    clip: clip.id,
                    effect: inst.id,
                    index: args.f64("index")?.max(0.0) as usize,
                }]
            }
        }
        "set_effect_keyframe" | "remove_effect_keyframe" => {
            let clip = clip_of(p, args)?;
            let (_, inst) = instance(clip, args.str("effect")?)?;
            let asset = fx::asset(p, &inst.fx)
                .ok_or_else(|| ToolError(format!("no FX program `{}`", inst.fx)))?;
            let param = args.str("param")?;
            let spec = asset
                .param(param)
                .ok_or_else(|| ToolError(format!("`{}` has no parameter `{param}`", asset.name)))?;
            let at = args.time("at")?;
            if tool == "set_effect_keyframe" {
                let value = args
                    .value("value")
                    .ok_or_else(|| ToolError("`value` is required".into()))?;
                vec![Command::SetEffectKeyframe {
                    clip: clip.id,
                    effect: inst.id,
                    param: param.to_owned(),
                    at,
                    value: param_value(spec, value)?,
                    ease: ease(args)?.unwrap_or_default(),
                }]
            } else {
                vec![Command::RemoveEffectKeyframe {
                    clip: clip.id,
                    effect: inst.id,
                    param: param.to_owned(),
                    at,
                }]
            }
        }
        "set_transition" => {
            let clip = clip_of(p, args)?;
            let edge = transition_edge(args)?;
            let current = match edge {
                Edge::Start => clip.transition_in.as_ref(),
                Edge::End => clip.transition_out.as_ref(),
            };
            let transition = match (args.opt_str("fx")?, current) {
                (Some(text), current) => {
                    let asset = fx_asset(p, text, Some(FxKind::Transition))?;
                    let keep = current.filter(|c| c.fx.fx == asset.id);
                    let mut t = keep.cloned().unwrap_or_else(|| {
                        Transition::new(
                            asset.id.clone(),
                            current.map_or(Time::from_secs_f64(TRANSITION), |c| c.duration),
                        )
                    });
                    t.fx.params = merged(&t.fx, asset, args)?;
                    t
                }
                (None, Some(current)) => {
                    let asset = fx::asset(p, &current.fx.fx)
                        .ok_or_else(|| ToolError("its program is missing".into()))?;
                    let mut t = current.clone();
                    t.fx.params = merged(&current.fx, asset, args)?;
                    t
                }
                (None, None) => {
                    return fail("give `fx` (e.g. cross_dissolve); there is no transition there");
                }
            };
            let mut transition = transition;
            if let Some(d) = args.opt_duration("duration")? {
                if d <= Time::ZERO {
                    return fail("`duration` must be positive");
                }
                transition.duration = d;
            }
            if let Some(a) = align(args)? {
                transition.align = a;
            }
            note = format!(
                "Reference: @transition {}/{}",
                clip.id,
                if edge == Edge::Start { "in" } else { "out" }
            );
            vec![Command::SetTransition {
                clip: clip.id,
                edge,
                transition: Some(transition),
            }]
        }
        "remove_transition" => {
            let clip = clip_of(p, args)?;
            vec![Command::SetTransition {
                clip: clip.id,
                edge: transition_edge(args)?,
                transition: None,
            }]
        }
        "add_generator" => return add_generator(ctx, args, &reason),
        "set_generator" => {
            let clip = clip_of(p, args)?;
            let ClipContent::Generator { fx: current } = &clip.content else {
                return fail(format!("{} is not a generator clip", clip.id));
            };
            let (asset, mut inst) = match args.opt_str("fx")? {
                Some(text) => {
                    let asset = fx_asset(p, text, Some(FxKind::Generator))?;
                    let mut inst = current.clone();
                    if inst.fx != asset.id {
                        inst.fx = asset.id.clone();
                        inst.params.clear();
                        inst.keyframes.clear();
                    }
                    (asset, inst)
                }
                None => (
                    fx::asset(p, &current.fx)
                        .ok_or_else(|| ToolError("its program is missing".into()))?,
                    current.clone(),
                ),
            };
            inst.params = merged(&inst, asset, args)?;
            vec![Command::SetGenerator {
                clip: clip.id,
                fx: inst,
            }]
        }
        other => return fail(format!("unknown tool `{other}`")),
    };
    let mut done = propose(ctx, commands, &reason);
    if !note.is_empty() && !done.result.is_error {
        append(&mut done, &note);
    }
    Ok(done.into())
}

fn transition_edge(args: &Args) -> ToolResultOf<Edge> {
    // `@transition c/in` in `clip` names the edge too.
    if !args.has("edge")
        && let Some(e) = args
            .opt_str("clip")?
            .and_then(|c| strip_token(c).split_once('/'))
            .map(|(_, e)| e.trim())
    {
        return match e {
            "in" | "start" => Ok(Edge::Start),
            "out" | "end" => Ok(Edge::End),
            _ => fail(format!("unknown edge `{e}`")),
        };
    }
    edge(args)
}

/// Add a line to a finished call's text.
fn append(done: &mut Done, line: &str) {
    if let Some(crate::bridge::Content::Text { text }) = done.result.content.first_mut() {
        text.push('\n');
        text.push_str(line);
    }
}

fn add_generator(ctx: &mut ToolCtx, args: &Args, reason: &str) -> Result<Step, ToolError> {
    let p = ctx.project();
    let asset = fx_asset(p, args.str("fx")?, Some(FxKind::Generator))?.clone();
    let range = args.range("range")?;
    let mut inst = FxInstance::new(asset.id.clone());
    inst.params = params(&asset, args.value("params"))?;
    let make = |inst: FxInstance| {
        move |_: &ToolCtx, track| {
            Ok(Command::AddGenerator {
                track,
                range,
                fx: inst,
            })
        }
    };
    // HTML motion graphics read as titles; shader pictures as overlays.
    // Without a track: the first of that kind free over the range, else a
    // new one on top.
    let (kind, letter) = match asset.language {
        FxLanguage::Html => (TrackKind::Title, "T"),
        FxLanguage::Glsl => (TrackKind::Overlay, "O"),
    };
    let free = p.timeline.tracks.iter().find(|t| {
        t.kind == kind && !t.locked && !t.clips.iter().any(|c| c.range().overlaps(&range))
    });
    let (commands, track) = match (args.has("track"), free) {
        (false, Some(t)) => (Vec::new(), Some(t.id)),
        (false, None) => {
            let n = p.timeline.tracks.iter().filter(|t| t.kind == kind).count() + 1;
            (
                vec![Command::AddTrack {
                    kind,
                    index: 0,
                    name: format!("{letter}{n}"),
                }],
                None,
            )
        }
        (true, _) => track_or_new(ctx, args, kind, letter)?,
    };
    let mut done = match track {
        Some(track) => {
            let command = make(inst)(ctx, track)?;
            let mut all = commands;
            all.push(command);
            propose(ctx, all, reason)
        }
        None => propose_then(ctx, commands, reason, kind, make(inst)),
    };
    if !done.result.is_error {
        let p = ctx.project();
        if let Some(clip) = p.timeline.tracks.iter().flat_map(|t| &t.clips).find(|c| {
            c.start == range.start
                && matches!(&c.content, ClipContent::Generator { fx } if fx.fx == asset.id)
        }) {
            append(&mut done, &format!("New clip: @clip {}", clip.id));
        }
    }
    Ok(done.into())
}

// ------------------------------------------------------------ authoring

/// `create_fx` / `update_fx`: validate, compile when an engine is present,
/// then propose `SetFxAsset`. Errors come back line-numbered so the agent
/// can fix them and call again.
pub fn author(ctx: &mut ToolCtx, tool: &str, args: &Args) -> Result<Step, ToolError> {
    let p = ctx.project();
    let mut asset = if tool == "create_fx" {
        let kind = match args.str("kind")? {
            "effect" => FxKind::Effect,
            "transition" => FxKind::Transition,
            "generator" => FxKind::Generator,
            k => return fail(format!("unknown kind `{k}`")),
        };
        let language = match args.opt_str("language")?.unwrap_or("glsl") {
            "glsl" => FxLanguage::Glsl,
            "html" => FxLanguage::Html,
            l => return fail(format!("unknown language `{l}`")),
        };
        FxAsset {
            id: FxRef::Project(FxId::new()),
            name: args.str("name")?.trim().to_owned(),
            kind,
            language,
            category: String::new(),
            description: String::new(),
            code: args.str("code")?.to_owned(),
            params: Vec::new(),
        }
    } else {
        let asset = fx_asset(p, args.str("fx")?, None)?;
        if !matches!(asset.id, FxRef::Project(_)) {
            return fail(
                "built-ins can't be changed; create_fx a copy (get_fx gives its code and params)",
            );
        }
        let mut asset = asset.clone();
        if let Some(name) = args.opt_str("name")? {
            asset.name = name.trim().to_owned();
        }
        if let Some(code) = args.opt_str("code")? {
            asset.code = code.to_owned();
        }
        asset
    };
    if let Some(c) = args.opt_str("category")? {
        asset.category = c.trim().to_owned();
    }
    if let Some(d) = args.opt_str("description")? {
        asset.description = d.trim().to_owned();
    }
    if let Some(v) = args.value("params") {
        asset.params = param_specs(v)?;
    }
    if let Err(e) = asset.validate() {
        return Ok(Done::error(format!("Not saved: {e}.")).into());
    }
    let compiled = match check(ctx, &asset) {
        Ok(compiled) => compiled,
        Err(errors) => {
            let mut done = Done::error(format!(
                "Not saved: the program does not compile. Fix these and call {tool} again:\n{errors}\n\nYour code with line numbers:\n{}",
                numbered(&asset.code)
            ));
            done.summary = format!("{tool} {}: compile errors", asset.name);
            return Ok(done.into());
        }
    };
    let reason = args
        .opt_str("reason")?
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            format!(
                "{} {} \"{}\"",
                if tool == "create_fx" { "New" } else { "Update" },
                kind_name(asset.kind),
                asset.name
            )
        });
    let id = asset.id.clone();
    let name = asset.name.clone();
    let mut done = propose(ctx, vec![Command::SetFxAsset { asset }], &reason);
    if !done.result.is_error {
        append(
            &mut done,
            &format!(
                "Program id: {id} (@fx {id}). {}\nIt is usable by id in this turn already; preview_fx it before applying.",
                if compiled {
                    "Compiled cleanly."
                } else {
                    "Validated structurally only: no engine for this language is available in this build, so it was not compiled."
                }
            ),
        );
        done.summary = format!("{tool} {name} → {id}");
    }
    Ok(done.into())
}

/// Compile with the engine for the asset's language. `Ok(false)` when no
/// engine is available; `Err` holds the engine's messages.
fn check(ctx: &ToolCtx, asset: &FxAsset) -> Result<bool, String> {
    match asset.language {
        FxLanguage::Glsl => match &ctx.services.fx {
            Some(engine) => engine.check(asset).map(|_| true).map_err(|e| e.0),
            None => Ok(false),
        },
        FxLanguage::Html => match &ctx.services.html {
            Some(engine) => {
                let values = FxInstance::new(asset.id.clone()).values_at(asset, Time::ZERO);
                let settings = &ctx.project().timeline.settings;
                let call = FxCall {
                    asset,
                    values: &values,
                    time: 0.5,
                    duration: 3.0,
                    canvas: (settings.width, settings.height),
                };
                match engine.check(&call) {
                    Ok(errors) if errors.is_empty() => Ok(true),
                    Ok(errors) => Err(errors.join("\n")),
                    Err(e) => Err(e.0),
                }
            }
            None => Ok(false),
        },
    }
}

fn numbered(code: &str) -> String {
    code.lines()
        .enumerate()
        .map(|(i, l)| format!("{:>3} | {l}", i + 1))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `params` of `create_fx`: `[{name, type, label?, default, min?, max?,
/// options?}]`.
fn param_specs(v: &Value) -> ToolResultOf<Vec<ParamSpec>> {
    let list = v
        .as_array()
        .ok_or_else(|| ToolError("`params` is a list of {name, type, default, …}".into()))?;
    list.iter()
        .map(|item| {
            let o = item
                .as_object()
                .ok_or_else(|| ToolError("each param is an object".into()))?;
            let s = |k: &str| o.get(k).and_then(Value::as_str);
            let n = |k: &str| o.get(k).and_then(Value::as_f64);
            let name = s("name").ok_or_else(|| ToolError("a param needs a `name`".into()))?;
            let ty = s("type").unwrap_or("float");
            let kind = match ty {
                "float" | "number" => ParamKind::Float {
                    min: n("min").unwrap_or(0.0),
                    max: n("max").unwrap_or(1.0),
                },
                "int" => ParamKind::Int {
                    min: n("min").unwrap_or(0.0) as i64,
                    max: n("max").unwrap_or(10.0) as i64,
                },
                "bool" => ParamKind::Bool,
                "color" => ParamKind::Color,
                "point" => ParamKind::Point,
                "text" => ParamKind::Text,
                "image" => ParamKind::Image,
                "choice" => ParamKind::Choice {
                    options: o
                        .get("options")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(Value::as_str)
                                .map(str::to_owned)
                                .collect()
                        })
                        .unwrap_or_default(),
                },
                t => {
                    return fail(format!(
                        "param `{name}`: unknown type `{t}` (float, int, bool, color, point, text, choice, image)"
                    ));
                }
            };
            let fallback = match &kind {
                ParamKind::Float { min, .. } => ParamValue::Number(*min),
                ParamKind::Int { min, .. } => ParamValue::Number(*min as f64),
                ParamKind::Bool => ParamValue::Bool(false),
                ParamKind::Color => ParamValue::List(vec![1.0, 1.0, 1.0, 1.0]),
                ParamKind::Point => ParamValue::List(vec![0.5, 0.5]),
                ParamKind::Text | ParamKind::Image => ParamValue::Text(String::new()),
                ParamKind::Choice { options } => {
                    ParamValue::Text(options.first().cloned().unwrap_or_default())
                }
            };
            let mut spec = ParamSpec {
                name: name.to_owned(),
                label: s("label").unwrap_or_default().to_owned(),
                kind,
                default: fallback,
            };
            if let Some(d) = o.get("default") {
                spec.default = param_value(&spec, d)
                    .map_err(|e| ToolError(format!("param `{name}` default: {}", e.0)))?;
            }
            Ok(spec)
        })
        .collect()
}

/// An asset as the agent reads it.
pub fn asset_json(project: &Project, asset: &FxAsset, code: bool, specs: bool) -> Value {
    let mut v = json!({
        "fx": asset.id.to_string(),
        "name": asset.name,
        "kind": kind_name(asset.kind),
        "language": language_name(asset.language),
        "builtin": matches!(asset.id, FxRef::Builtin(_)),
    });
    if !asset.category.is_empty() {
        v["category"] = json!(asset.category);
    }
    if !asset.description.is_empty() {
        v["description"] = json!(asset.description);
    }
    if specs {
        v["params"] = asset.params.iter().map(param_spec_json).collect();
    } else {
        v["params"] = json!(
            asset
                .params
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
        );
    }
    if code {
        v["code"] = json!(asset.code);
        let used: Vec<_> = fx::uses(project, &asset.id)
            .iter()
            .map(|c| c.to_string())
            .collect();
        v["used_by_clips"] = json!(used);
    }
    v
}

pub fn param_spec_json(p: &ParamSpec) -> Value {
    let mut v = serde_json::to_value(p).unwrap_or_default();
    if let Some(o) = v.as_object_mut()
        && o.get("label").and_then(Value::as_str) == Some("")
    {
        o.remove("label");
    }
    v
}

/// An instance's values (non-default params and keyframes) for reads.
pub fn instance_json(project: &Project, inst: &FxInstance, origin: Time) -> Value {
    let mut v = json!({
        "id": inst.id.to_string(),
        "fx": inst.fx.to_string(),
    });
    if let Some(a) = fx::asset(project, &inst.fx) {
        v["name"] = json!(a.name);
    }
    if !inst.enabled {
        v["enabled"] = json!(false);
    }
    if !inst.params.is_empty() {
        v["params"] = json!(inst.params);
    }
    if !inst.keyframes.is_empty() {
        v["keyframes"] = inst
            .keyframes
            .iter()
            .map(|k| {
                json!({
                    "param": k.param, "at": secs(origin + k.at), "value": k.value, "ease": k.ease,
                })
            })
            .collect();
    }
    v
}

/// Overrides for previews: the instance's values with `params` on top.
pub fn preview_values(
    asset: &FxAsset,
    base: Option<&FxInstance>,
    overrides: Option<&Value>,
) -> ToolResultOf<FxInstance> {
    let mut inst = base
        .cloned()
        .unwrap_or_else(|| FxInstance::new(asset.id.clone()));
    inst.params.extend(params(asset, overrides)?);
    Ok(inst)
}
