//! Controls for FX parameters, generated from [`ParamSpec`]: number slider
//! and field (Float/Int), switch (Bool), swatch with picker (Color), x/y
//! fields (Point), text box (Text), dropdown (Choice) and file chooser
//! (Image), each with keyframe navigation at the playhead. Like
//! [`super::param`], drags preview live and commit once on release.
//!
//! A control is keyed by what owns the parameter ([`FxOwner`]); the owner
//! decides the command that sets it, so other inspector sections (audio
//! effects, grade) can reuse the controls by adding an owner.

use super::InspectorPanel;
use super::param::format_value;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, h_flex, v_flex};
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
    numbers: Vec<Number>,
    color: Option<Entity<ColorPickerState>>,
    text: Option<Entity<TextareaState>>,
    /// Last model value shown; `None` resyncs on the next render.
    shown: Option<ParamValue>,
    _subscriptions: Vec<Subscription>,
}

/// A number with slider (Float/Int) or a bare field (Point components).
struct Number {
    slider: Option<Entity<SliderState>>,
    field: Entity<InputState>,
    decimals: usize,
    dragging: bool,
    typing: bool,
}

/// Decimals and slider step for a range.
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
                subs.push(cx.subscribe_in(
                    &state,
                    window,
                    move |this, _, ev: &ColorPickerEvent, _, cx| {
                        let ColorPickerEvent::Change(Some(c)) = ev else {
                            return;
                        };
                        let rgb = c.to_rgb();
                        let v = [rgb.r, rgb.g, rgb.b, rgb.a].map(|c| c as f64).to_vec();
                        this.set_fx_param(&k, ParamValue::List(v), false, cx);
                    },
                ));
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
                subs.push(cx.subscribe_in(
                    &state,
                    window,
                    move |this, input, ev: &InputEvent, _, cx| {
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
                    },
                ));
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

    /// Follow the model, except where the user is dragging or typing.
    fn sync(&mut self, value: &ParamValue, window: &mut Window, cx: &mut App) {
        if self.shown.as_ref() == Some(value) {
            return;
        }
        let parts: Vec<f64> = match value {
            ParamValue::List(v) => v.clone(),
            other => other.as_f64().into_iter().collect(),
        };
        if self.numbers.iter().any(|n| n.dragging) {
            return;
        }
        self.shown = Some(value.clone());
        for (n, v) in self.numbers.iter_mut().zip(&parts) {
            if let Some(slider) = &n.slider {
                slider.update(cx, |s, cx| s.set_value(*v as f32, window, cx));
            }
            if !n.typing {
                let t = format_value(*v as f32, n.decimals);
                n.field.update(cx, |f, cx| f.set_value(t, window, cx));
            }
        }
        if let Some(color) = &self.color {
            let c = to_hsla(Some(&parts));
            color.update(cx, |s, cx| s.set_value(c, window, cx));
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

/// A number field, with a slider when `range` is given. `part` is the
/// component of a Point.
fn number(
    key: &ControlKey,
    part: usize,
    range: Option<(f64, f64, bool)>,
    value: f64,
    window: &mut Window,
    cx: &mut Context<InspectorPanel>,
    subs: &mut Vec<Subscription>,
) -> Number {
    let (min, max, int) = range.unwrap_or((0.0, 1.0, false));
    let (decimals, step) = precision(min, max, int);
    let slider = range.map(|_| {
        let slider = cx.new(|_| {
            SliderState::new()
                .min(min as f32)
                .max(max as f32)
                .step(step as f32)
                .default_value(value as f32)
        });
        let k = key.clone();
        subs.push(cx.subscribe_in(
            &slider,
            window,
            move |this, _, ev: &SliderEvent, window, cx| match ev {
                SliderEvent::Change(v) => {
                    let v = v.start() as f64;
                    if let Some(n) = this.number_mut(&k, part) {
                        n.dragging = true;
                        let t = format_value(v as f32, n.decimals);
                        n.field.update(cx, |f, cx| f.set_value(t, window, cx));
                    }
                    let value = this.with_part(&k, part, v);
                    this.set_fx_param(&k, value, true, cx);
                }
                SliderEvent::Release(v) => {
                    if let Some(n) = this.number_mut(&k, part) {
                        n.dragging = false;
                    }
                    let value = this.with_part(&k, part, v.start() as f64);
                    this.set_fx_param(&k, value, false, cx);
                }
            },
        ));
        slider
    });
    let field = cx
        .new(|cx| InputState::new(window, cx).default_value(format_value(value as f32, decimals)));
    let k = key.clone();
    subs.push(cx.subscribe_in(
        &field,
        window,
        move |this, input, ev: &InputEvent, _, cx| {
            if let Some(n) = this.number_mut(&k, part) {
                n.typing = matches!(ev, InputEvent::Change);
            }
            if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                let Ok(v) = input.read(cx).value().trim().parse::<f64>() else {
                    this.invalidate_control(&k);
                    cx.notify();
                    return;
                };
                let v = if range.is_some() {
                    v.clamp(min, max)
                } else {
                    v
                };
                let value = this.with_part(&k, part, v);
                let current = this.fx_controls.get(&k).and_then(|c| c.shown.clone());
                if current.as_ref() != Some(&value) {
                    this.set_fx_param(&k, value, false, cx);
                }
            }
        },
    ));
    Number {
        slider,
        field,
        decimals,
        dragging: false,
        typing: false,
    }
}

impl InspectorPanel {
    fn number_mut(&mut self, key: &ControlKey, part: usize) -> Option<&mut Number> {
        self.fx_controls.get_mut(key)?.numbers.get_mut(part)
    }

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
        let control = match self.fx_controls.entry(key.clone()) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(Control::new(&key, spec, &value, window, cx))
            }
        };
        control.sync(&value, window, cx);
        let control = &self.fx_controls[&key];
        let muted = cx.theme().muted_foreground;
        let id = format!("{:?}-{}", key.owner, key.param);
        let reset = (key.clone(), spec.default.clone());
        let label = div()
            .id(SharedString::from(format!("label-{id}")))
            .w(px(64.))
            .flex_none()
            .text_xs()
            .text_color(muted)
            .truncate()
            .child(spec.label().to_owned())
            .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                if ev.click_count() >= 2 {
                    this.set_fx_param(&reset.0, reset.1.clone(), false, cx);
                }
            }));
        let keyable = !matches!(
            spec.kind,
            ParamKind::Text | ParamKind::Choice { .. } | ParamKind::Image
        );
        let keys = self.fx_key_nav(target, &spec.name, &value, keyable, cx);
        let row = h_flex().min_h(px(28.)).gap_2().child(label);
        let entity = cx.entity();
        match &spec.kind {
            ParamKind::Float { .. } | ParamKind::Int { .. } => {
                let n = &control.numbers[0];
                row.children(
                    n.slider
                        .as_ref()
                        .map(|s| div().flex_1().min_w_0().child(Slider::new(s).w_full())),
                )
                .child(
                    div()
                        .w(px(56.))
                        .flex_none()
                        .child(Input::new(&n.field).xsmall()),
                )
                .child(keys)
                .into_any_element()
            }
            ParamKind::Point => row
                .children(control.numbers.iter().map(|n| {
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Input::new(&n.field).xsmall())
                }))
                .child(keys)
                .into_any_element(),
            ParamKind::Bool => {
                let on = value.as_bool().unwrap_or(false);
                let k = key.clone();
                row.child(
                    div().flex_1().child(
                        Switch::new(SharedString::from(format!("switch-{id}")))
                            .small()
                            .checked(on)
                            .on_click(move |v, _, cx| {
                                let v = *v;
                                entity.update(cx, |this, cx| {
                                    this.set_fx_param(&k, ParamValue::Bool(v), false, cx)
                                })
                            }),
                    ),
                )
                .child(keys)
                .into_any_element()
            }
            ParamKind::Color => row
                .child(
                    div()
                        .flex_1()
                        .children(control.color.as_ref().map(|c| ColorPicker::new(c).small())),
                )
                .child(keys)
                .into_any_element(),
            ParamKind::Text => v_flex()
                .gap_1()
                .child(row.child(div().flex_1()).child(keys))
                .children(control.text.as_ref().map(|t| Textarea::new(t).small()))
                .into_any_element(),
            ParamKind::Choice { options } => {
                let current = value.as_text().unwrap_or_default().to_owned();
                let options = options.clone();
                let picked = current.clone();
                row.child(
                    div().flex_1().min_w_0().child(
                        Button::new(SharedString::from(format!("choice-{id}")))
                            .outline()
                            .xsmall()
                            .w_full()
                            .label(current)
                            .dropdown_caret(true)
                            .dropdown_menu(move |mut menu, _, _| {
                                for option in &options {
                                    let (entity, k, o) =
                                        (entity.clone(), key.clone(), option.clone());
                                    menu = menu.item(
                                        PopupMenuItem::new(option.clone())
                                            .checked(*option == picked)
                                            .on_click(move |_, _, cx| {
                                                entity.update(cx, |this, cx| {
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
                    ),
                )
                .child(keys)
                .into_any_element()
            }
            ParamKind::Image => {
                let path = value.as_text().unwrap_or_default().to_owned();
                let name = std::path::Path::new(&path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let k = key.clone();
                row.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_xs()
                        .truncate()
                        .when(name.is_empty(), |d| d.text_color(muted).child("—"))
                        .child(name),
                )
                .child(
                    Button::new(SharedString::from(format!("image-{id}")))
                        .ghost()
                        .xsmall()
                        .icon(IconName::FolderOpen)
                        .tooltip("Choose image")
                        .on_click(cx.listener(move |this, _, _, cx| this.choose_image(&k, cx))),
                )
                .child(keys)
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

    /// Previous keyframe, add/remove at the playhead, next keyframe. The
    /// arrows show only on animated parameters; space is kept so rows align.
    fn fx_key_nav(
        &self,
        target: &FxTarget,
        param: &str,
        value: &ParamValue,
        keyable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let (primary, muted, hover) =
            (theme.primary, theme.muted_foreground, theme.secondary_hover);
        let group = h_flex().flex_none().w(px(42.)).justify_end();
        if !keyable {
            return group.into_any_element();
        }
        let keyed = target.keyed_at(param);
        let animated = target.animated(param);
        let inside = target.inside;
        let color = if animated { primary } else { muted };
        let id = format!("{:?}-{param}", target.owner);
        let (clip, effect, origin) = (target.owner.clip(), target.instance.id, target.origin);
        let times: Vec<Time> = {
            let mut t: Vec<Time> = target
                .instance
                .keyframes
                .iter()
                .filter(|k| k.param == param)
                .map(|k| k.at)
                .collect();
            t.sort();
            t
        };
        let prev = times
            .iter()
            .rev()
            .find(|t| **t < target.at - Time(target.frame.0 / 2))
            .copied();
        let next = times
            .iter()
            .find(|t| **t > target.at + Time(target.frame.0 / 2))
            .copied();
        let arrow = |name: &str, glyph: IconName, to: Option<Time>, cx: &mut Context<Self>| {
            div()
                .id(SharedString::from(format!("{name}-{id}")))
                .w(px(12.))
                .h(px(18.))
                .flex()
                .items_center()
                .justify_center()
                .text_color(muted)
                .when(!animated, |d| d.invisible())
                .when(to.is_none(), |d| d.opacity(0.35))
                .when_some(to, |d, to| {
                    d.cursor_pointer()
                        .hover(|s| s.text_color(primary))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state.update(cx, |s, cx| s.seek(origin + to, cx));
                        }))
                })
                .child(gpui_kit::component::Icon::new(glyph).xsmall())
        };
        let (param_name, value) = (param.to_owned(), value.clone());
        let diamond = div()
            .id(SharedString::from(format!("kf-{id}")))
            .flex_none()
            .size(px(18.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_sm()
            .when(inside, |d| d.cursor_pointer().hover(|s| s.bg(hover)))
            .when(!inside, |d| d.opacity(0.4))
            .child(
                div()
                    .size(px(7.))
                    .rounded(px(1.5))
                    .border_1()
                    .border_color(color)
                    .when(keyed.is_some(), |d| d.bg(color)),
            )
            .tooltip(|window, cx| Tooltip::new("Keyframe").build(window, cx))
            .when(inside, |d| {
                d.on_click(cx.listener(move |this, _, _, cx| {
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
                }))
            });
        group
            .child(arrow("prev", IconName::ChevronLeft, prev, cx))
            .child(diamond)
            .child(arrow("next", IconName::ChevronRight, next, cx))
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
