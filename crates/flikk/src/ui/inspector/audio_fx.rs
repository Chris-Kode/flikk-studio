//! A clip's sound for the inspector: volume with its keyframe lane at the
//! playhead (volume automation), Clean voice with its amount (and how far
//! its cleaned copy is made), and its effect stack with parameters. The
//! inspector embeds it and sets the clip each render.

use super::{group, row};
use crate::state::EditorState;
use crate::ui::kit::theme as t;
use crate::ui::kit::{
    Button, DropdownMenu as _, Icon, IconName, Keyframe, KeyframeNav, NumberField, PopupMenuItem,
    ScrubLabel, SectionHeader, Spinner, Switch, Tooltip, h_flex, v_flex,
};
use crate::ui::mixer::controls::{self, ControlHost, Controls, Ctl, Range, Target};
use crate::ui::mixer::stack;
use flikk_core::audiofx::AudioFx;
use flikk_core::cleanup::{Progress, VoiceCleanup};
use flikk_core::command::Command;
use flikk_core::model::{Clip, Property};
use flikk_core::{ClipId, EffectId, Time};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::HashSet;

const VOLUME: Range = Range {
    min: -48.,
    max: 24.,
    log: false,
};
const AMOUNT: Range = Range {
    min: 0.,
    max: 100.,
    log: false,
};
/// How often the progress of a cleaned copy is read while it is made.
const PROGRESS_POLL: std::time::Duration = std::time::Duration::from_millis(250);

/// What can be folded away.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Fold {
    Clip,
    Voice,
    Effects,
    Effect(EffectId),
}

pub struct ClipAudioFx {
    state: Entity<EditorState>,
    controls: Controls,
    clip: Option<ClipId>,
    folded: HashSet<Fold>,
    /// A redraw is due to show a cleaned copy's progress.
    polling: bool,
    _observe: Subscription,
}

impl ControlHost for ClipAudioFx {
    fn controls(&mut self) -> &mut Controls {
        &mut self.controls
    }

    fn editor(&self) -> &Entity<EditorState> {
        &self.state
    }
}

impl ClipAudioFx {
    pub fn new(state: Entity<EditorState>, cx: &mut Context<Self>) -> Self {
        let _observe = cx.observe(&state, |_, _, cx| cx.notify());
        Self {
            state,
            controls: Controls::default(),
            clip: None,
            folded: HashSet::new(),
            polling: false,
            _observe,
        }
    }

    pub fn set_clip(&mut self, clip: Option<ClipId>, cx: &mut Context<Self>) {
        if self.clip != clip {
            self.clip = clip;
            cx.notify();
        }
    }

    /// The volume keyframe at the playhead, within half a frame.
    fn key_at(clip: &Clip, playhead: Time, half_frame: Time) -> Option<Time> {
        let rel = controls::clip_time(clip, playhead);
        clip.keyframes
            .iter()
            .filter(|k| k.property == Property::Volume)
            .find(|k| (k.at - rel).0.abs() <= half_frame.0)
            .map(|k| k.at)
    }

    fn toggle_key(state: &Entity<EditorState>, clip: Option<ClipId>, cx: &mut App) {
        let editor = state.clone();
        let state = state.read(cx);
        let (project, playhead) = (state.bus.project(), state.playhead);
        let Some((_, clip)) = clip.and_then(|id| project.timeline.clip(id)) else {
            return;
        };
        let half = Time(project.timeline.settings.rate.frame_duration().0 / 2);
        let command = match Self::key_at(clip, playhead, half) {
            Some(at) => Command::RemoveKeyframe {
                clip: clip.id,
                property: Property::Volume,
                at: clip.start + at,
            },
            None => {
                let rel = controls::clip_time(clip, playhead);
                Command::SetKeyframe {
                    clip: clip.id,
                    property: Property::Volume,
                    at: clip.start + rel,
                    value: clip.value_at(Property::Volume, rel),
                    ease: Default::default(),
                }
            }
        };
        editor.update(cx, |s, cx| {
            s.execute(command, cx);
        });
    }

    fn is_open(&self, fold: Fold) -> bool {
        !self.folded.contains(&fold)
    }

    /// Fold or unfold `fold`.
    fn toggler(&self, fold: Fold, cx: &Context<Self>) -> impl Fn(&mut App) + 'static {
        let view = cx.entity().downgrade();
        move |cx| {
            let _ = view.update(cx, |this, cx| {
                if !this.folded.remove(&fold) {
                    this.folded.insert(fold);
                }
                cx.notify();
            });
        }
    }

    /// A group's collapsible header.
    fn header(
        &self,
        id: &'static str,
        title: &'static str,
        fold: Fold,
        cx: &Context<Self>,
    ) -> SectionHeader {
        let toggle = self.toggler(fold, cx);
        SectionHeader::new(id, title)
            .open(self.is_open(fold))
            .on_toggle(move |_, _, cx| toggle(cx))
    }

    /// The volume row: its label scrubs, the lane adds or removes a
    /// keyframe at the playhead and seeks to the neighbouring ones.
    fn volume_row(
        &mut self,
        clip: &Clip,
        playhead: Time,
        half: Time,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let rel = controls::clip_time(clip, playhead);
        let keyed = Self::key_at(clip, playhead, half).is_some();
        let times: Vec<Time> = clip
            .keyframes
            .iter()
            .filter(|k| k.property == Property::Volume)
            .map(|k| k.at)
            .collect();
        let keyframe = match (keyed, times.is_empty()) {
            (true, _) => Keyframe::On,
            (false, false) => Keyframe::Off,
            (false, true) => Keyframe::None,
        };
        let inside = playhead >= clip.start && playhead < clip.start + clip.duration;
        let prev = times.iter().copied().filter(|t| *t < rel - half).max();
        let next = times.iter().copied().filter(|t| *t > rel + half).min();
        let start = clip.start;
        let seek = |to: Option<Time>| {
            let state = self.state.clone();
            to.map(|to| {
                move |_: &mut Window, cx: &mut App| state.update(cx, |s, cx| s.seek(start + to, cx))
            })
        };
        let (prev, next) = (seek(prev), seek(next));
        let (key_state, key_clip) = (self.state.clone(), self.clip);
        let nav = KeyframeNav::new("volume-key", keyframe)
            .disabled(!inside)
            .on_toggle(move |_, cx| Self::toggle_key(&key_state, key_clip, cx))
            .when_some(prev, |nav, f| nav.on_prev(f))
            .when_some(next, |nav, f| nav.on_next(f));

        let volume = clip.value_at(Property::Volume, rel);
        let ctl = Ctl::ClipVolume(clip.id);
        let field = controls::field(self, ctl.clone(), volume, VOLUME, 1, window, cx);
        let state = self.state.clone();
        let label = ScrubLabel::new(&field, "Volume")
            .keyed(keyed)
            .on_reset(move |_, cx| controls::commit(&state, &ctl, 0., cx));
        row(
            label,
            NumberField::new(&field).keyed(keyed).unit("dB"),
            Some(nav.into_any_element()),
        )
    }

    /// Clean voice: its switch (disabled without its model), how far the
    /// cleaned copy is while it is made, and the amount while on.
    fn voice_group(&mut self, clip: &Clip, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let target = Target::Clip(clip.id);
        let (ready, progress) = {
            let state = self.state.read(cx);
            (
                controls::voice_ready(state),
                controls::voice_progress(state, target),
            )
        };
        let voice = clip.audio.voice_cleanup.unwrap_or(VoiceCleanup {
            enabled: false,
            amount: VoiceCleanup::DEFAULT_AMOUNT,
        });
        if matches!(progress, Some(Progress::Making(_))) && !self.polling {
            self.polling = true;
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(PROGRESS_POLL).await;
                let _ = this.update(cx, |this, cx| {
                    this.polling = false;
                    cx.notify();
                });
            })
            .detach();
        }
        let status = match &progress {
            Some(Progress::Making(p)) => Some(
                h_flex()
                    .gap(px(4.))
                    .child(Spinner::new().size(px(12.)))
                    .child(
                        div()
                            .font_family(t::mono())
                            .text_size(px(10.))
                            .text_color(t::T3)
                            .child(format!("{:.0}%", p * 100.)),
                    )
                    .into_any_element(),
            ),
            Some(Progress::Failed(why) | Progress::Unavailable(why)) => Some(
                div()
                    .id("clean-voice-failed")
                    .child(
                        Icon::new(IconName::CircleAlert)
                            .size(px(14.))
                            .color(t::WARN),
                    )
                    .tooltip(Tooltip::text(why.clone()))
                    .into_any_element(),
            ),
            Some(Progress::Made(_)) | None => None,
        };
        let state = self.state.clone();
        let switch = Switch::new("clean-voice-on")
            .checked(voice.enabled)
            .disabled(!ready)
            .on_click(move |on: &bool, _, cx| controls::set_voice(&state, target, *on, cx));
        let header = self
            .header("clean-voice", "Clean voice", Fold::Voice, cx)
            .when_some(status, |h, status| h.child(status))
            .child(switch);
        let rows = if voice.enabled && self.is_open(Fold::Voice) {
            let ctl = Ctl::CleanVoice(target);
            let amount = voice.amount * 100.;
            let field = controls::field(self, ctl.clone(), amount, AMOUNT, 0, window, cx);
            let state = self.state.clone();
            let label = ScrubLabel::new(&field, "Amount").on_reset(move |_, cx| {
                controls::commit(&state, &ctl, VoiceCleanup::DEFAULT_AMOUNT * 100., cx)
            });
            vec![row(label, NumberField::new(&field).unit("%"), None)]
        } else {
            Vec::new()
        };
        group(header, rows)
    }

    /// One effect: its header (fold, name, enable, menu), then its
    /// parameters unless folded.
    fn effect(
        &mut self,
        i: usize,
        target: Target,
        fx: &AudioFx,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let open = self.is_open(Fold::Effect(fx.id));
        let toggle = self.toggler(Fold::Effect(fx.id), cx);
        let (on_state, rm_state, id) = (self.state.clone(), self.state.clone(), fx.id);
        let menu = Button::new(("clip-fx-menu", i))
            .ghost()
            .xsmall()
            .icon(IconName::Ellipsis)
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                let state = rm_state.clone();
                menu.item(
                    PopupMenuItem::new("Remove")
                        .icon(IconName::Trash)
                        .destructive()
                        .on_click(move |_, _, cx| stack::remove(&state, target, id, cx)),
                )
            });
        let header = h_flex()
            .id(("clip-fx", i))
            .flex_none()
            .h(px(26.))
            .gap(px(6.))
            .pl(px(18.))
            .pr(px(6.))
            .cursor_pointer()
            .on_click(move |_, _, cx| toggle(cx))
            .child(
                Icon::new(if open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .size(px(12.))
                .color(t::T3),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(if fx.enabled { t::T1 } else { t::T3 })
                    .child(fx.kind.label()),
            )
            .child(
                // The controls' clicks are theirs, not the fold's.
                h_flex()
                    .gap(px(4.))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        Switch::new(("clip-fx-on", i))
                            .checked(fx.enabled)
                            .tooltip(if fx.enabled { "Bypass" } else { "Enable" })
                            .on_click(move |_, _, cx| stack::toggle(&on_state, target, id, cx)),
                    )
                    .child(menu),
            );
        let rows = if open {
            self.param_rows(target, fx, window, cx)
        } else {
            Vec::new()
        };
        v_flex().child(header).children(rows)
    }

    /// A row per parameter of `fx`: label (scrubs; double-click resets),
    /// field and unit.
    fn param_rows(
        &mut self,
        target: Target,
        fx: &AudioFx,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Div> {
        let defaults = AudioFx::new(fx.kind);
        fx.kind
            .params()
            .into_iter()
            .map(|spec| {
                let ctl = Ctl::Param(target, fx.id, spec.name.clone());
                let range = controls::param_range(fx, &spec.name);
                let (label, unit) = controls::split_label(&spec.label);
                let decimals = controls::decimals(unit, range);
                let value = fx.value(&spec.name) as f32;
                let field = controls::field(self, ctl.clone(), value, range, decimals, window, cx);
                let default = defaults.value(&spec.name) as f32;
                let state = self.state.clone();
                let label = ScrubLabel::new(&field, label.to_owned())
                    .on_reset(move |_, cx| controls::commit(&state, &ctl, default, cx));
                let number =
                    NumberField::new(&field).when(!unit.is_empty(), |f| f.unit(unit.to_owned()));
                row(label, number, None)
            })
            .collect()
    }
}

impl Render for ClipAudioFx {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.controls.prune();
        let state = self.state.read(cx);
        let project = state.bus.project();
        let playhead = state.playhead;
        let half = Time(project.timeline.settings.rate.frame_duration().0 / 2);
        let Some(clip) = self
            .clip
            .and_then(|id| project.timeline.clip(id))
            .map(|(_, c)| c.clone())
        else {
            return v_flex();
        };
        let target = Target::Clip(clip.id);

        let volume = if self.is_open(Fold::Clip) {
            vec![self.volume_row(&clip, playhead, half, window, cx)]
        } else {
            Vec::new()
        };
        let effects = if self.is_open(Fold::Effects) {
            let mut rows = Vec::new();
            for (i, fx) in clip.audio.effects.iter().enumerate() {
                rows.push(self.effect(i, target, fx, window, cx));
            }
            rows
        } else {
            Vec::new()
        };
        let add = stack::add_menu(
            Button::new("clip-add-fx")
                .ghost()
                .xsmall()
                .icon(IconName::Plus)
                .tooltip("Add effect"),
            &self.state,
            target,
        );
        let effects = group(
            self.header("clip-effects", "Effects", Fold::Effects, cx)
                .child(add),
            effects,
        );
        let voice = self.voice_group(&clip, window, cx);
        v_flex()
            .child(group(
                self.header("clip-volume", "Clip", Fold::Clip, cx),
                volume,
            ))
            .child(voice)
            .child(stack::drop_target(
                div().id("clip-fx-drop").child(effects),
                &self.state,
                target,
            ))
    }
}
