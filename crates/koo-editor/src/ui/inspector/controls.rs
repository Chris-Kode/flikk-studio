//! Controls for FX parameters, generated from [`ParamSpec`]: a scrubbing
//! number field (Float/Int), x/y fields (Point), switch (Bool), colour field
//! (Color), text box (Text), select (Choice) and file chooser (Image), each
//! with keyframe navigation at the playhead. Like [`super::param`], scrubs
//! preview live and commit once on release.
//!
//! A control is keyed by what owns the parameter ([`FxOwner`]); the owner
//! decides the command that sets it, so other inspector sections (audio
//! effects, grade) can reuse the controls by adding an owner.

use super::{InspectorPanel, label, row, select_field};
use crate::ui::kit::theme as t;
use crate::ui::kit::{
    Button, ColorField, DropdownMenu as _, IconName, Keyframe, KeyframeNav, NumberField,
    NumberFieldEvent, NumberFieldState, PopupMenuItem, ScrubLabel, Switch, Textarea, h_flex,
    v_flex,
};
use gpui_kit::base::input::{InputEvent, TextareaState};
use gpui_kit::base::{ColorPickerEvent, ColorPickerState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::command::{Command, Edge};
use koo_core::fx::{self, FxAsset, FxInstance, ParamKind, ParamSpec, ParamValue, Transition};
use koo_core::model::{ClipContent, Project};
use koo_core::{ClipId, EffectId, Time};

/// What a parameter belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FxOwner {
    /// An effect on a clip's stack.
    Effect { clip: ClipId, effect: EffectId },
    /// A generator clip's program.
    Generator { clip: ClipId },
    /// The transition at a clip's start or end.
    Transition { clip: ClipId, edge: Edge },
}

impl std::hash::Hash for FxOwner {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            Self::Effect { clip, effect } => (clip, effect).hash(state),
            Self::Generator { clip } => clip.hash(state),
            Self::Transition { clip, edge } => (clip, matches!(edge, Edge::Start)).hash(state),
        }
    }
}

impl FxOwner {
    pub fn clip(&self) -> ClipId {
        match self {
            Self::Effect { clip, .. }
            | Self::Generator { clip }
            | Self::Transition { clip, .. } => *clip,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ControlKey {
    pub owner: FxOwner,
    pub param: String,
}

/// An owner resolved against the project at the playhead.
#[derive(Clone)]
pub struct FxTarget {
    pub owner: FxOwner,
    pub instance: FxInstance,
    pub asset: FxAsset,
    /// The transition holding `instance`, for transition owners.
    pub transition: Option<Transition>,
    /// Timeline time where the instance's clock (and keyframes) start.
    pub origin: Time,
    /// Instance time at the playhead, clamped into its span.
    pub at: Time,
    /// The playhead is inside the instance's span.
    pub inside: bool,
    pub frame: Time,
}

impl FxTarget {
    pub fn resolve(project: &Project, owner: &FxOwner, playhead: Time) -> Option<Self> {
        let tl = &project.timeline;
        let (track, clip) = tl.clip(owner.clip())?;
        let (instance, transition, span) = match owner {
            FxOwner::Effect { effect, .. } => {
                let e = clip.effects.iter().find(|e| e.id == *effect)?;
                (e.clone(), None, clip.range())
            }
            FxOwner::Generator { .. } => match &clip.content {
                ClipContent::Generator { fx } => (fx.clone(), None, clip.range()),
                _ => return None,
            },
            FxOwner::Transition { edge, .. } => {
                let t = match edge {
                    Edge::Start => clip.transition_in.as_ref(),
                    Edge::End => clip.transition_out.as_ref(),
                }?;
                // Where it plays; a transition the neighbour overrides still
                // edits against its nominal span.
                let span = fx::transitions(track)
                    .into_iter()
                    .find(|p| p.transition.fx.id == t.fx.id)
                    .map(|p| p.range)
                    .unwrap_or_else(|| match edge {
                        Edge::Start => {
                            koo_core::TimeRange::new(clip.start, clip.start + t.duration)
                        }
                        Edge::End => koo_core::TimeRange::new(clip.end() - t.duration, clip.end()),
                    });
                (t.fx.clone(), Some(t.clone()), span)
            }
        };
        let asset = fx::asset(project, &instance.fx)?.clone();
        let last = Time((span.duration().0 - 1).max(0));
        Some(Self {
            owner: owner.clone(),
            instance,
            asset,
            transition,
            origin: span.start,
            at: (playhead - span.start).clamp(Time::ZERO, last),
            inside: span.contains(playhead),
            frame: tl.settings.rate.frame_duration(),
        })
    }

    pub fn values(&self) -> std::collections::BTreeMap<String, ParamValue> {
        self.instance.values_at(&self.asset, self.at)
    }

    fn keyed_at(&self, param: &str) -> Option<Time> {
        self.instance
            .keyframes
            .iter()
            .filter(|k| k.param == param)
            .map(|k| k.at)
            .find(|at| (*at - self.at).0.abs() <= self.frame.0 / 2)
    }

    fn animated(&self, param: &str) -> bool {
        self.instance.keyframes.iter().any(|k| k.param == param)
    }

    /// The command that stores `instance` back on its owner.
    pub fn store(&self, instance: FxInstance) -> Command {
        match &self.owner {
            FxOwner::Effect { clip, .. } => Command::SetEffect {
                clip: *clip,
                effect: instance,
            },
            FxOwner::Generator { clip } => Command::SetGenerator {
                clip: *clip,
                fx: instance,
            },
            FxOwner::Transition { clip, edge } => Command::SetTransition {
                clip: *clip,
                edge: *edge,
                transition: self
                    .transition
                    .clone()
                    .map(|t| Transition { fx: instance, ..t }),
            },
        }
    }

    /// Set `param` to `value`: a keyframe at the playhead when the parameter
    /// is animated, else its static value.
    pub fn set(&self, param: &str, value: ParamValue) -> Command {
        if self.animated(param) {
            return Command::SetEffectKeyframe {
                clip: self.owner.clip(),
                effect: self.instance.id,
                param: param.to_owned(),
                at: self.origin + self.at,
                value,
                ease: Default::default(),
            };
        }
        let mut instance = self.instance.clone();
        if self.asset.param(param).is_some_and(|s| s.default == value) {
            instance.params.remove(param);
        } else {
            instance.params.insert(param.to_owned(), value);
        }
        self.store(instance)
    }
}

/// Widgets of one parameter, created on first use.
pub struct Control {
    /// One field for a number, two for a point.
    numbers: Vec<Entity<NumberFieldState>>,
    color: Option<Entity<ColorPickerState>>,
    text: Option<Entity<TextareaState>>,
    /// Last model value shown; `None` resyncs on the next render.
    shown: Option<ParamValue>,
    _subscriptions: Vec<Subscription>,
}

/// Decimals and step for a range.
fn precision(min: f64, max: f64, int: bool) -> (usize, f64) {
    if int {
        return (0, 1.0);
    }
    let span = (max - min).abs();
    let decimals = if span <= 1.0 {
        3
    } else if span <= 10.0 {
        2
    } else if span <= 100.0 {
        1
    } else {
        0
    };
    (decimals, 10f64.powi(-(decimals as i32)))
}

impl Control {
    fn new(
        key: &ControlKey,
        spec: &ParamSpec,
        value: &ParamValue,
        window: &mut Window,
        cx: &mut Context<InspectorPanel>,
    ) -> Self {
        let mut subs = Vec::new();
        let mut numbers = Vec::new();
        let mut color = None;
        let mut text = None;
        match &spec.kind {
            ParamKind::Float { .. } | ParamKind::Int { .. } => {
                let (min, max, int) = match spec.kind {
                    ParamKind::Int { min, max } => (min as f64, max as f64, true),
                    ParamKind::Float { min, max } => (min, max, false),
                    _ => unreachable!(),
                };
                let v = value.as_f64().unwrap_or(min);
                numbers.push(number(
                    key,
                    0,
                    Some((min, max, int)),
                    v,
                    window,
                    cx,
                    &mut subs,
                ));
            }
            ParamKind::Point => {
                let p = value.as_list().unwrap_or(&[0.5, 0.5]).to_vec();
                for i in 0..2 {
                    let v = p.get(i).copied().unwrap_or(0.5);
                    numbers.push(number(key, i, None, v, window, cx, &mut subs));
                }
            }
            ParamKind::Color => {
                let state = cx.new(|cx| {
                    ColorPickerState::new(window, cx).default_value(to_hsla(value.as_list()))
                });
                let k = key.clone();
                subs.push(
                    cx.subscribe(&state, move |this, _, ev: &ColorPickerEvent, cx| {
                        let ColorPickerEvent::Change(Some(c)) = ev else {
                            return;
                        };
                        let rgb = c.to_rgb();
                        let v = [rgb.r, rgb.g, rgb.b, rgb.a].map(|c| c as f64).to_vec();
                        this.set_fx_param(&k, ParamValue::List(v), false, cx);
                    }),
                );
                color = Some(state);
            }
            ParamKind::Text => {
                let state = cx.new(|cx| {
                    TextareaState::new(window, cx)
                        .auto_grow(1, 6)
                        .submit_on_enter(true)
                        .default_value(value.as_text().unwrap_or_default().to_owned())
                });
                let k = key.clone();
                subs.push(
                    cx.subscribe(&state, move |this, input, ev: &InputEvent, cx| {
                        if matches!(
                            ev,
                            InputEvent::PressEnter { shift: false, .. } | InputEvent::Blur
                        ) {
                            let typed = input.read(cx).value().to_string();
                            let current = this.fx_controls.get(&k).and_then(|c| c.shown.clone());
                            if current.as_ref().and_then(|v| v.as_text()) != Some(typed.as_str()) {
                                this.set_fx_param(&k, ParamValue::Text(typed), false, cx);
                            }
                        }
                    }),
                );
                text = Some(state);
            }
            _ => {}
        }
        Self {
            numbers,
            color,
            text,
            shown: Some(value.clone()),
            _subscriptions: subs,
        }
    }

    /// Follow the model; a scrub or an edit in progress keeps its value.
    fn sync(&mut self, value: &ParamValue, window: &mut Window, cx: &mut App) {
        if self.shown.as_ref() == Some(value) {
            return;
        }
        self.shown = Some(value.clone());
        let parts: Vec<f64> = match value {
            ParamValue::List(v) => v.clone(),
            other => other.as_f64().into_iter().collect(),
        };
        for (n, v) in self.numbers.iter().zip(&parts) {
            n.update(cx, |n, cx| n.set_value(*v, cx));
        }
        if let Some(color) = &self.color {
            let c = to_hsla(Some(&parts));
            color.update(cx, |s, cx| {
                if s.value() != Some(c) {
                    s.set_value(c, window, cx);
                }
            });
        }
        if let (Some(text), Some(t)) = (&self.text, value.as_text()) {
            let editing = text.read(cx).focus_handle(cx).is_focused(window);
            if !editing {
                text.update(cx, |s, cx| s.set_value(t.to_owned(), window, cx));
            }
        }
    }
}

fn to_hsla(rgba: Option<&[f64]>) -> Hsla {
    let c = rgba.unwrap_or(&[1.0, 1.0, 1.0, 1.0]);
    let at = |i: usize, d: f64| c.get(i).copied().unwrap_or(d) as f32;
    Rgba {
        r: at(0, 1.0),
        g: at(1, 1.0),
        b: at(2, 1.0),
        a: at(3, 1.0),
    }
    .into()
}

/// A number field, clamped to `range` when given. `part` is the component
/// of a Point.
fn number(
    key: &ControlKey,
    part: usize,
    range: Option<(f64, f64, bool)>,
    value: f64,
    window: &mut Window,
    cx: &mut Context<InspectorPanel>,
    subs: &mut Vec<Subscription>,
) -> Entity<NumberFieldState> {
    let (min, max, int) = range.unwrap_or((0.0, 1.0, false));
    let (decimals, step) = precision(min, max, int);
    let field = cx.new(|cx| {
        let field = NumberFieldState::new(window, cx)
            .step(step)
            .decimals(decimals)
            .default_value(value);
        match range {
            Some(_) => field.min(min).max(max),
            None => field,
        }
    });
    let k = key.clone();
    subs.push(
        cx.subscribe(&field, move |this, _, ev: &NumberFieldEvent, cx| {
            let (v, live) = match *ev {
                NumberFieldEvent::Change(v) => (v, true),
                NumberFieldEvent::Commit(v) => (v, false),
            };
            let value = this.with_part(&k, part, v);
            this.set_fx_param(&k, value, live, cx);
        }),
    );
    field
}

impl InspectorPanel {
    fn invalidate_control(&mut self, key: &ControlKey) {
        if let Some(c) = self.fx_controls.get_mut(key) {
            c.shown = None;
        }
    }

    /// The parameter's current value with component `part` replaced (Point),
    /// or just `v`.
    fn with_part(&self, key: &ControlKey, part: usize, v: f64) -> ParamValue {
        match self
            .fx_controls
            .get(key)
            .and_then(|c| c.shown.clone())
            .and_then(|s| s.as_list().map(<[f64]>::to_vec))
        {
            Some(mut list) => {
                if let Some(slot) = list.get_mut(part) {
                    *slot = v;
                }
                ParamValue::List(list)
            }
            None => ParamValue::Number(v),
        }
    }

    pub(super) fn fx_target(&self, owner: &FxOwner, cx: &App) -> Option<FxTarget> {
        let state = self.state.read(cx);
        FxTarget::resolve(state.bus.project(), owner, state.playhead)
    }

    /// Set a parameter: shown live while `live`, else committed.
    pub(super) fn set_fx_param(
        &mut self,
        key: &ControlKey,
        value: ParamValue,
        live: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.fx_target(&key.owner, cx) else {
            return;
        };
        let command = target.set(&key.param, value);
        if live {
            self.state.update(cx, |s, cx| s.show_live(&[command], cx));
            return;
        }
        self.invalidate_control(key);
        self.state.update(cx, |s, cx| s.show_live(&[], cx));
        self.run(vec![command], "set_effect", cx);
    }

    /// One row per parameter of `target`'s program.
    pub(super) fn fx_param_rows(
        &mut self,
        target: &FxTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let values = target.values();
        target
            .asset
            .params
            .iter()
            .map(|spec| {
                let value = values
                    .get(&spec.name)
                    .cloned()
                    .unwrap_or(spec.default.clone());
                self.fx_param_row(target, spec, value, window, cx)
            })
            .collect()
    }

    fn fx_param_row(
        &mut self,
        target: &FxTarget,
        spec: &ParamSpec,
        value: ParamValue,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = ControlKey {
            owner: target.owner.clone(),
            param: spec.name.clone(),
        };
        self.fx_controls
            .entry(key.clone())
            .or_insert_with(|| Control::new(&key, spec, &value, window, cx))
            .sync(&value, window, cx);
        let control = &self.fx_controls[&key];
        let id = format!("{:?}-{}", key.owner, key.param);
        let keyable = !matches!(
            spec.kind,
            ParamKind::Text | ParamKind::Choice { .. } | ParamKind::Image
        );
        let keyed = target.keyed_at(&spec.name).is_some();
        let lane = keyable.then(|| self.fx_key_nav(target, &spec.name, &value, cx));
        let panel = cx.entity();
        let reset = {
            let (panel, key, default) = (panel.downgrade(), key.clone(), spec.default.clone());
            move |_: &mut Window, cx: &mut App| {
                let _ = panel.update(cx, |this, cx| {
                    this.set_fx_param(&key, default.clone(), false, cx)
                });
            }
        };
        let name = spec.label().to_owned();
        match &spec.kind {
            ParamKind::Float { .. } | ParamKind::Int { .. } => {
                let n = &control.numbers[0];
                row(
                    ScrubLabel::new(n, name).keyed(keyed).on_reset(reset),
                    NumberField::new(n).keyed(keyed),
                    lane,
                )
                .into_any_element()
            }
            ParamKind::Point => {
                let fields = control.numbers.iter().zip(["X", "Y"]).map(|(n, axis)| {
                    div()
                        .relative()
                        .flex_1()
                        .min_w_0()
                        .child(NumberField::new(n).keyed(keyed).pl(px(18.)))
                        .child(
                            div()
                                .absolute()
                                .left(px(6.))
                                .top_0()
                                .bottom_0()
                                .flex()
                                .items_center()
                                .occlude()
                                .child(ScrubLabel::new(n, axis).keyed(keyed)),
                        )
                });
                row(label(name), h_flex().gap(px(4.)).children(fields), lane).into_any_element()
            }
            ParamKind::Bool => {
                let on = value.as_bool().unwrap_or(false);
                let k = key.clone();
                row(
                    label(name),
                    Switch::new(SharedString::from(format!("switch-{id}")))
                        .checked(on)
                        .on_click(move |v, _, cx| {
                            let v = *v;
                            panel.update(cx, |this, cx| {
                                this.set_fx_param(&k, ParamValue::Bool(v), false, cx)
                            })
                        }),
                    lane,
                )
                .into_any_element()
            }
            ParamKind::Color => row(
                label(name),
                div().children(
                    control
                        .color
                        .as_ref()
                        .map(|c| ColorField::new(SharedString::from(format!("color-{id}")), c)),
                ),
                lane,
            )
            .into_any_element(),
            ParamKind::Text => v_flex()
                .child(row(label(name), div(), None))
                .children(control.text.as_ref().map(|t| {
                    div()
                        .pl(px(22.))
                        .pr(px(10.))
                        .pb(px(4.))
                        .child(Textarea::new(t).small())
                }))
                .into_any_element(),
            ParamKind::Choice { options } => {
                let current = value.as_text().unwrap_or_default().to_owned();
                let options = options.clone();
                let picked = current.clone();
                row(
                    label(name),
                    select_field(SharedString::from(format!("choice-{id}")), current)
                        .dropdown_menu(move |mut menu, _, _| {
                            for option in &options {
                                let (panel, k, o) = (panel.clone(), key.clone(), option.clone());
                                menu = menu.item(
                                    PopupMenuItem::new(option.clone())
                                        .checked(*option == picked)
                                        .on_click(move |_, _, cx| {
                                            panel.update(cx, |this, cx| {
                                                this.set_fx_param(
                                                    &k,
                                                    ParamValue::Text(o.clone()),
                                                    false,
                                                    cx,
                                                )
                                            })
                                        }),
                                );
                            }
                            menu
                        }),
                    None,
                )
                .into_any_element()
            }
            ParamKind::Image => {
                let path = value.as_text().unwrap_or_default().to_owned();
                let file = std::path::Path::new(&path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned());
                let k = key.clone();
                row(
                    label(name),
                    h_flex()
                        .gap(px(4.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(11.))
                                .text_color(if file.is_some() { t::T1 } else { t::T3 })
                                .child(file.unwrap_or_else(|| "\u{2014}".into())),
                        )
                        .child(
                            Button::new(SharedString::from(format!("image-{id}")))
                                .ghost()
                                .xsmall()
                                .icon(IconName::FolderOpen)
                                .tooltip("Choose image")
                                .on_click(move |_, _, cx| {
                                    panel.update(cx, |this, cx| this.choose_image(&k, cx))
                                }),
                        ),
                    None,
                )
                .into_any_element()
            }
        }
    }

    fn choose_image(&mut self, key: &ControlKey, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        let key = key.clone();
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await
                && let Some(path) = paths.into_iter().next()
            {
                let value = ParamValue::Text(path.to_string_lossy().into_owned());
                this.update(cx, |this, cx| this.set_fx_param(&key, value, false, cx))
                    .ok();
            }
        })
        .detach();
    }

    /// Previous keyframe, add or remove one at the playhead, next keyframe.
    fn fx_key_nav(
        &self,
        target: &FxTarget,
        param: &str,
        value: &ParamValue,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let keyed = target.keyed_at(param);
        let state = match (keyed, target.animated(param)) {
            (Some(_), _) => Keyframe::On,
            (None, true) => Keyframe::Off,
            (None, false) => Keyframe::None,
        };
        let (clip, effect, origin) = (target.owner.clip(), target.instance.id, target.origin);
        let mut times: Vec<Time> = target
            .instance
            .keyframes
            .iter()
            .filter(|k| k.param == param)
            .map(|k| k.at)
            .collect();
        times.sort();
        let half = Time(target.frame.0 / 2);
        let prev = times.iter().rev().find(|t| **t < target.at - half).copied();
        let next = times.iter().find(|t| **t > target.at + half).copied();
        let panel = cx.entity();
        let seek = |to: Option<Time>| {
            let panel = panel.clone();
            to.map(|to| {
                move |_: &mut Window, cx: &mut App| {
                    panel.update(cx, |this, cx| {
                        this.state.update(cx, |s, cx| s.seek(origin + to, cx))
                    })
                }
            })
        };
        let (prev, next) = (seek(prev), seek(next));
        let (param_name, value) = (param.to_owned(), value.clone());
        KeyframeNav::new(
            SharedString::from(format!("kf-{:?}-{param}", target.owner)),
            state,
        )
        .disabled(!target.inside)
        .on_toggle(move |_, cx| {
            panel.update(cx, |this, cx| {
                let command = match keyed {
                    Some(at) => Command::RemoveEffectKeyframe {
                        clip,
                        effect,
                        param: param_name.clone(),
                        at: origin + at,
                    },
                    None => {
                        let Some(t) = this.fx_target_by_clip(clip, effect, cx) else {
                            return;
                        };
                        Command::SetEffectKeyframe {
                            clip,
                            effect,
                            param: param_name.clone(),
                            at: t.origin + t.at,
                            value: value.clone(),
                            ease: Default::default(),
                        }
                    }
                };
                this.run(vec![command], "keyframe", cx);
            })
        })
        .when_some(prev, |nav, f| nav.on_prev(f))
        .when_some(next, |nav, f| nav.on_next(f))
        .into_any_element()
    }

    /// Re-resolve an instance by its id at click time (the playhead may have
    /// moved since the render).
    fn fx_target_by_clip(&self, clip: ClipId, effect: EffectId, cx: &App) -> Option<FxTarget> {
        let state = self.state.read(cx);
        let (_, c) = state.bus.project().timeline.clip(clip)?;
        let owner = if c.effects.iter().any(|e| e.id == effect) {
            FxOwner::Effect { clip, effect }
        } else if matches!(&c.content, ClipContent::Generator { fx } if fx.id == effect) {
            FxOwner::Generator { clip }
        } else if c.transition_in.as_ref().is_some_and(|t| t.fx.id == effect) {
            FxOwner::Transition {
                clip,
                edge: Edge::Start,
            }
        } else {
            FxOwner::Transition {
                clip,
                edge: Edge::End,
            }
        };
        FxTarget::resolve(state.bus.project(), &owner, state.playhead)
    }
}
