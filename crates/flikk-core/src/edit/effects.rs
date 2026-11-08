//! Effect stacks, generators, transitions and their keyframes.

use super::keyframes::{key_tolerance, nearest_key};
use super::ops;
use super::target::{editable_clip, find_clip};
use super::update::update_clip;
use crate::command::{Edge, Plan};
use crate::error::{CoreError, Result};
use crate::fx::{self, FxAsset, FxInstance, FxKind, FxRef, ParamKeyframe, ParamValue, Transition};
use crate::ids::*;
use crate::model::*;
use crate::ops::Op;
use crate::time::Time;

/// The asset `r` names, which must be of `kind`.
pub(super) fn fx_asset<'a>(project: &'a Project, r: &FxRef, kind: FxKind) -> Result<&'a FxAsset> {
    let asset =
        fx::asset(project, r).ok_or_else(|| CoreError::Invalid(format!("no FX program `{r}`")))?;
    if asset.kind != kind {
        return Err(CoreError::Invalid(format!(
            "`{}` is a {}, not a {}",
            asset.name,
            asset.kind.label().to_lowercase(),
            kind.label().to_lowercase()
        )));
    }
    Ok(asset)
}

/// `fx` with unknown parameters dropped and values coerced to their kinds.
pub(super) fn coerced(fx: &FxInstance, asset: &FxAsset) -> FxInstance {
    let mut out = fx.clone();
    out.params = fx
        .params
        .iter()
        .filter_map(|(name, v)| {
            let spec = asset.param(name)?;
            Some((name.clone(), spec.kind.coerce(v, &spec.default)))
        })
        .collect();
    out.keyframes = fx
        .keyframes
        .iter()
        .filter_map(|k| {
            let spec = asset.param(&k.param)?;
            Some(ParamKeyframe {
                value: spec.kind.coerce(&k.value, &spec.default),
                ..k.clone()
            })
        })
        .collect();
    out
}

/// Whether the clip shows a picture effects can process.
fn has_picture(track: &Track, clip: &Clip) -> bool {
    track.kind != TrackKind::Audio && !matches!(clip.content, ClipContent::Caption { .. })
}

pub(super) fn add_effect(
    project: &Project,
    clips: &[ClipId],
    effect: &FxInstance,
    index: Option<usize>,
) -> Result<Plan> {
    let tl = &project.timeline;
    let asset = fx_asset(project, &effect.fx, FxKind::Effect)?;
    if clips.is_empty() {
        return Err(CoreError::Invalid("no clips given".into()));
    }
    let effect = coerced(effect, asset);
    let mut out = Vec::new();
    for (i, id) in clips.iter().enumerate() {
        let (track, clip) = find_clip(tl, *id)?;
        if !has_picture(track, clip) {
            return Err(CoreError::Invalid(format!(
                "{id} has no picture to apply an effect to"
            )));
        }
        out.extend(
            update_clip(tl, *id, |c| {
                let mut e = effect.clone();
                // The first clip keeps the caller's id so it can refer to it.
                if i > 0 || c.effects.iter().any(|x| x.id == e.id) {
                    e.id = EffectId::new();
                }
                let at = index.unwrap_or(c.effects.len()).min(c.effects.len());
                c.effects.insert(at, e);
            })?
            .ops,
        );
    }
    Ok(ops(out))
}

/// Edit the effect stack of `clip` where effect `effect` is at `ix`.
pub(super) fn with_effect(
    tl: &Timeline,
    clip: ClipId,
    effect: EffectId,
    f: impl FnOnce(&mut Vec<FxInstance>, usize),
) -> Result<Plan> {
    let (_, c) = find_clip(tl, clip)?;
    let ix = c
        .effects
        .iter()
        .position(|e| e.id == effect)
        .ok_or_else(|| CoreError::Invalid(format!("{clip} has no effect {effect}")))?;
    update_clip(tl, clip, |c| f(&mut c.effects, ix))
}

/// Set (`Some`) or remove (`None`, nearest within half a frame) a keyframe
/// on an effect, generator or transition instance of `clip`.
pub(super) fn set_effect_keyframe(
    project: &Project,
    clip: ClipId,
    effect: EffectId,
    param: &str,
    at: Time,
    key: Option<(&ParamValue, crate::model::Ease)>,
) -> Result<Plan> {
    let tl = &project.timeline;
    let (track, c) = find_clip(tl, clip)?;
    // The instance's clock origin in timeline time.
    let origin = if c.effects.iter().any(|e| e.id == effect)
        || matches!(&c.content, ClipContent::Generator { fx } if fx.id == effect)
    {
        c.start
    } else {
        fx::transitions(track)
            .into_iter()
            .find(|p| p.transition.fx.id == effect)
            .map(|p| p.range.start)
            .ok_or_else(|| CoreError::Invalid(format!("{clip} has no effect {effect}")))?
    };
    let rel = at - origin;
    let tolerance = key_tolerance(tl);
    let mut after = c.clone();
    let inst = instance_mut(&mut after, effect)
        .ok_or_else(|| CoreError::Invalid(format!("{clip} has no effect {effect}")))?;
    let asset = fx::asset(project, &inst.fx)
        .ok_or_else(|| CoreError::Invalid(format!("no FX program `{}`", inst.fx)))?;
    let spec = asset.param(param).ok_or_else(|| {
        CoreError::Invalid(format!("`{}` has no parameter `{param}`", asset.name))
    })?;
    match key {
        Some((value, ease)) => {
            if !spec.kind.accepts(value) && spec.kind.coerce(value, &spec.default) == spec.default {
                return Err(CoreError::Invalid(format!(
                    "value does not fit parameter `{param}`"
                )));
            }
            let value = spec.kind.coerce(value, &spec.default);
            inst.keyframes
                .retain(|k| !(k.param == param && (k.at - rel).0.abs() <= tolerance.0));
            inst.keyframes.push(ParamKeyframe {
                param: param.to_owned(),
                at: rel,
                value,
                ease,
            });
            inst.keyframes.sort_by_key(|k| k.at);
        }
        None => {
            let ix = nearest_key(
                inst.keyframes
                    .iter()
                    .map(|k| (k.param == param).then_some(k.at)),
                rel,
                tolerance,
            )
            .ok_or_else(|| CoreError::Invalid(format!("no `{param}` keyframe at {at}")))?;
            inst.keyframes.remove(ix);
        }
    }
    Ok(ops(vec![Op::UpdateClip {
        track: track.id,
        before: Box::new(c.clone()),
        after: Box::new(after),
    }]))
}

/// The effect, generator or transition instance `id` of a clip.
fn instance_mut(c: &mut Clip, id: EffectId) -> Option<&mut FxInstance> {
    c.fx_instances_mut().find(|i| i.id == id)
}

pub(super) fn set_transition(
    project: &Project,
    id: ClipId,
    edge: Edge,
    transition: Option<&Transition>,
) -> Result<Plan> {
    let tl = &project.timeline;
    let (track, clip) = editable_clip(tl, id)?;
    if matches!(clip.content, ClipContent::Caption { .. }) {
        return Err(CoreError::Invalid("captions take no transitions".into()));
    }
    let transition = match transition {
        Some(t) => {
            let asset = fx_asset(project, &t.fx.fx, FxKind::Transition)?;
            if t.duration <= Time::ZERO {
                return Err(CoreError::Invalid("a transition needs a duration".into()));
            }
            Some(Transition {
                fx: coerced(&t.fx, asset),
                ..t.clone()
            })
        }
        None => None,
    };
    let mut plan = update_clip(tl, id, |c| match edge {
        Edge::Start => c.transition_in = transition,
        Edge::End => c.transition_out = transition,
    })?;
    // One transition per cut: the clip it touches there lets go of its
    // own, so this one is what plays and removing it leaves a plain cut.
    let neighbour = track.clips.iter().find(|n| {
        n.id != id
            && n.enabled
            && match edge {
                Edge::Start => n.end() == clip.start,
                Edge::End => n.start == clip.end(),
            }
    });
    if let Some(n) = neighbour {
        plan.ops.extend(
            update_clip(tl, n.id, |n| match edge {
                Edge::Start => n.transition_out = None,
                Edge::End => n.transition_in = None,
            })?
            .ops,
        );
    }
    Ok(plan)
}
