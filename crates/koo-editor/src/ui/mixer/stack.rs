//! Effect inserts shared by the mixer strips and the inspector: add from a
//! menu or by dropping an [`AudioFxDrag`], enable, remove, and parameter
//! rows. Every change is one `SetTrackMix` or `SetAudioEffects`.

use super::controls::{self, ControlHost, Ctl, Target};
use crate::state::EditorState;
use crate::ui::AudioFxDrag;
use crate::ui::library::shared;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::slider::{Slider, SliderState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, h_flex};
use gpui_kit::*;
use koo_core::audiofx::{AudioFx, AudioFxKind};
use koo_core::library::Preset;
use koo_core::{EffectId, LibraryId};

/// Tooltip builder for plain elements.
pub fn tip(text: &'static str) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    move |window, cx| Tooltip::new(text).build(window, cx)
}

/// Run `f` on `target`'s stack as one command.
pub fn edit(
    state: &Entity<EditorState>,
    target: Target,
    f: impl FnOnce(&mut Vec<AudioFx>),
    cx: &mut App,
) {
    let command = controls::edit_effects(state.read(cx).bus.project(), target, f);
    if let Some(command) = command {
        state.update(cx, |s, cx| {
            s.execute(command, cx);
        });
    }
}

pub fn add(state: &Entity<EditorState>, target: Target, kind: AudioFxKind, cx: &mut App) {
    edit(state, target, |fx| fx.push(AudioFx::new(kind)), cx);
}

pub fn toggle(state: &Entity<EditorState>, target: Target, id: EffectId, cx: &mut App) {
    edit(
        state,
        target,
        |fx| {
            if let Some(f) = fx.iter_mut().find(|f| f.id == id) {
                f.enabled = !f.enabled;
            }
        },
        cx,
    );
}

pub fn remove(state: &Entity<EditorState>, target: Target, id: EffectId, cx: &mut App) {
    edit(state, target, |fx| fx.retain(|f| f.id != id), cx);
}

/// "+" with a menu of every effect, the audio presets, and saving the
/// stack as a preset.
pub fn add_button(
    id: impl Into<ElementId>,
    state: &Entity<EditorState>,
    target: Target,
) -> impl IntoElement {
    let state = state.clone();
    Button::new(id)
        .ghost()
        .xsmall()
        .icon(IconName::Plus)
        .tooltip("Add effect")
        .dropdown_menu(move |mut menu, _, cx| {
            for kind in AudioFxKind::ALL {
                let state = state.clone();
                menu = menu.item(
                    PopupMenuItem::new(kind.label())
                        .on_click(move |_, _, cx| add(&state, target, kind, cx)),
                );
            }
            let s = state.read(cx);
            let presets: Vec<_> = s
                .library
                .presets
                .iter()
                .filter(|p| p.has_sound())
                .map(|p| (p.id, p.name.clone()))
                .collect();
            if !presets.is_empty() {
                menu = menu
                    .item(PopupMenuItem::separator())
                    .item(PopupMenuItem::label("Presets"));
            }
            for (id, name) in presets {
                let state = state.clone();
                menu = menu.item(
                    PopupMenuItem::new(name)
                        .on_click(move |_, _, cx| add_preset(&state, target, id, cx)),
                );
            }
            let stack = stack_of(state.read(cx), target);
            if !stack.is_empty() {
                let state = state.clone();
                menu = menu.item(PopupMenuItem::separator()).item(
                    PopupMenuItem::new("Save as Preset").on_click(move |_, _, cx| {
                        state.update(cx, |s, cx| {
                            let name = s.library.free_preset_name("Audio");
                            let preset = Preset::audio(&name, stack.clone());
                            shared::save_preset(s, preset, cx);
                        })
                    }),
                );
            }
            menu
        })
}

/// `target`'s effect stack.
fn stack_of(s: &EditorState, target: Target) -> Vec<AudioFx> {
    let tl = &s.bus.project().timeline;
    match target {
        Target::Track(track) => tl.track(track).map(|t| t.mix.effects.clone()),
        Target::Clip(clip) => tl.clip(clip).map(|(_, c)| c.audio.effects.clone()),
    }
    .unwrap_or_default()
}

/// Add an audio preset's effects after the others.
fn add_preset(state: &Entity<EditorState>, target: Target, id: LibraryId, cx: &mut App) {
    let Some(preset) = state.read(cx).library.preset(id).cloned() else {
        return;
    };
    edit(
        state,
        target,
        |fx| {
            fx.extend(preset.audio.iter().map(|a| AudioFx {
                id: EffectId::new(),
                ..a.clone()
            }))
        },
        cx,
    );
}

/// Accept dropped effects on `el`.
pub fn drop_target<E: InteractiveElement>(
    el: E,
    state: &Entity<EditorState>,
    target: Target,
    cx: &App,
) -> E {
    let tint = cx.theme().accent.opacity(0.35);
    let state = state.clone();
    el.drag_over::<AudioFxDrag>(move |s, _, _, _| s.bg(tint))
        .on_drop(move |drag: &AudioFxDrag, _, cx| add(&state, target, drag.kind, cx))
}

/// One parameter: label, slider and readout.
pub struct ParamRow {
    pub label: SharedString,
    pub slider: Entity<SliderState>,
    pub value: SharedString,
}

/// The rows of `fx`'s parameters, creating their sliders.
pub fn param_rows<T: ControlHost>(
    host: &mut T,
    target: Target,
    fx: &AudioFx,
    window: &mut Window,
    cx: &mut Context<T>,
) -> Vec<ParamRow> {
    fx.kind
        .params()
        .into_iter()
        .map(|spec| {
            let ctl = Ctl::Param(target, fx.id, spec.name.clone());
            let range = controls::param_range(fx, &spec.name);
            let slider = controls::knob(
                host,
                ctl.clone(),
                fx.value(&spec.name) as f32,
                range,
                window,
                cx,
            );
            let (label, unit) = controls::split_label(&spec.label);
            let shown = controls::shown(host, &ctl).unwrap_or(fx.value(&spec.name) as f32);
            ParamRow {
                label: label.to_owned().into(),
                slider,
                value: controls::format(shown, unit).into(),
            }
        })
        .collect()
}

/// A parameter row as an element, 28 px high.
pub fn param_row(row: &ParamRow, muted: Hsla) -> Div {
    h_flex()
        .h(px(28.))
        .gap_2()
        .child(
            div()
                .w(px(76.))
                .flex_none()
                .text_xs()
                .text_color(muted)
                .truncate()
                .child(row.label.clone()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(Slider::new(&row.slider).w_full()),
        )
        .child(
            div()
                .w(px(40.))
                .flex_none()
                .text_xs()
                .text_right()
                .text_color(muted)
                .child(row.value.clone()),
        )
}
