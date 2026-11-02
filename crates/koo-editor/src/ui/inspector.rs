//! Inspector, contextual by workspace: in Edit, the selected transition, or
//! the selected clip's properties by kind with its effects, grade and linked
//! audio; in Color and Audio, the whole timeline's grade, or its mixer,
//! loudness and ducking.

mod audio;
mod clip;
mod color;
mod controls;
mod edit;
mod effects;
mod generator;
mod masks;
mod param;
mod stabilize;
mod transition;

pub(crate) use clip::sound_clip;
pub(crate) use param::format_value;

use crate::state::EditorState;
use crate::ui::caption_style::{self, CaptionStylePanel};
use clip::PictureTab;
use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::*;
use koo_core::agent::Workspace;
use koo_core::analysis::Loudness;
use koo_core::command::{Command, Edge};
use koo_core::model::{Clip, ClipContent, Ducking, Project, Property, Track};
use koo_core::{ClipId, Time};
use param::{Key, Param};
use std::collections::{HashMap, HashSet};

pub struct InspectorPanel {
    state: Entity<EditorState>,
    params: HashMap<Key, Param>,
    /// Clip whose grade "paste" applies to the selection.
    copied_grade: Option<ClipId>,
    /// Integrated loudness and true peak per clip, before clip gain.
    loudness: HashMap<ClipId, Loudness>,
    measuring: bool,
    lufs_target: Entity<InputState>,
    peak_target: Entity<InputState>,
    picture_tab: PictureTab,
    /// Caption or title text, and the clip it was loaded from.
    text: Entity<TextareaState>,
    text_clip: Option<ClipId>,
    /// Caption or title start and end, and the clip they show.
    start_field: Entity<InputState>,
    end_field: Entity<InputState>,
    timing_clip: Option<ClipId>,
    /// Style of the selected caption's track.
    caption_style: Entity<CaptionStylePanel>,
    /// FX parameter controls, by owner and parameter.
    fx_controls: HashMap<controls::ControlKey, controls::Control>,
    /// Effects folded to their header.
    collapsed_fx: HashSet<koo_core::EffectId>,
    /// Clip name field while renaming, and the clip it renames.
    name_field: Entity<InputState>,
    renaming: Option<ClipId>,
    /// Speed in percent, the clip it shows, and whether a change ripples.
    speed_field: Entity<InputState>,
    speed_clip: Option<ClipId>,
    speed_ripple: bool,
    _subscriptions: Vec<Subscription>,
}

impl InspectorPanel {
    pub fn new(state: Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let text = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 8)
                .submit_on_enter(true)
        });
        let start_field = cx.new(|cx| InputState::new(window, cx));
        let name_field = cx.new(|cx| InputState::new(window, cx));
        let speed_field = cx.new(|cx| InputState::new(window, cx));
        let end_field = cx.new(|cx| InputState::new(window, cx));
        let commit = |ev: &InputEvent| {
            matches!(
                ev,
                InputEvent::PressEnter { shift: false, .. } | InputEvent::Blur
            )
        };
        let _subscriptions = vec![
            cx.observe(&state, |_, _, cx| cx.notify()),
            cx.subscribe(&text, move |this, _, ev: &InputEvent, cx| {
                if commit(ev) {
                    this.commit_text(cx);
                }
            }),
            cx.subscribe(&start_field, move |this, _, ev: &InputEvent, cx| {
                if commit(ev) {
                    this.commit_timing(Edge::Start, cx);
                }
            }),
            cx.subscribe(&end_field, move |this, _, ev: &InputEvent, cx| {
                if commit(ev) {
                    this.commit_timing(Edge::End, cx);
                }
            }),
            cx.subscribe(&name_field, move |this, _, ev: &InputEvent, cx| {
                if commit(ev) {
                    this.commit_rename(cx);
                }
            }),
            cx.subscribe(&speed_field, move |this, _, ev: &InputEvent, cx| {
                if commit(ev) {
                    this.commit_speed(cx);
                }
            }),
        ];
        let caption_style = cx.new(|cx| {
            CaptionStylePanel::new(state.clone(), caption_style::Target::Active, window, cx)
        });
        let presets = state.read(cx).presets.clone();
        let lufs_target = cx.new(|cx| {
            InputState::new(window, cx).default_value(param::format_value(presets.loudness_lufs, 1))
        });
        let peak_target = cx.new(|cx| {
            InputState::new(window, cx).default_value(param::format_value(presets.true_peak_db, 1))
        });
        Self {
            state,
            params: HashMap::new(),
            copied_grade: None,
            loudness: HashMap::new(),
            measuring: false,
            lufs_target,
            peak_target,
            picture_tab: match crate::demo::fx_view().as_deref() {
                Some("effects") => PictureTab::Effects,
                _ => PictureTab::default(),
            },
            text,
            text_clip: None,
            start_field,
            end_field,
            timing_clip: None,
            caption_style,
            fx_controls: HashMap::new(),
            collapsed_fx: HashSet::new(),
            name_field,
            renaming: None,
            speed_field,
            speed_clip: None,
            speed_ripple: false,
            _subscriptions,
        }
    }

    /// The control for `key`, created on first use and synced to `value`.
    fn param(
        &mut self,
        key: Key,
        value: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> &Param {
        let p = match self.params.entry(key) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(Param::new(key, value, window, cx))
            }
        };
        p.sync(value, window, cx);
        p
    }

    fn project<'a>(&self, cx: &'a App) -> &'a Project {
        self.state.read(cx).bus.project()
    }

    /// Selected clips that still exist, in selection order.
    fn selected_clips(&self, cx: &App) -> Vec<Clip> {
        let state = self.state.read(cx);
        let tl = &state.bus.project().timeline;
        state
            .selection
            .clips
            .iter()
            .filter_map(|id| tl.clip(*id).map(|(_, c)| c.clone()))
            .collect()
    }

    fn run(&self, commands: Vec<Command>, label: &str, cx: &mut Context<Self>) {
        if commands.is_empty() {
            return;
        }
        self.state.update(cx, |s, cx| {
            if commands.len() == 1 {
                s.execute(commands.into_iter().next().unwrap(), cx);
            } else {
                s.execute_batch(commands, label, cx);
            }
        });
    }

    /// Apply a control's new model value.
    fn commit(&mut self, key: Key, v: f32, cx: &mut Context<Self>) {
        if let Some(p) = self.params.get_mut(&key) {
            p.invalidate();
        }
        self.state.update(cx, |s, cx| s.show_live(&[], cx));
        let (commands, label) = self.commands(key, v, cx);
        self.run(commands, label, cx);
    }

    /// Show a value in the viewer while its slider is dragged.
    fn preview(&mut self, key: Key, v: f32, cx: &mut Context<Self>) {
        let (commands, _) = self.commands(key, v, cx);
        self.state.update(cx, |s, cx| s.show_live(&commands, cx));
    }

    /// The commands that set `key` to `v` on the selection.
    fn commands(&self, key: Key, v: f32, cx: &App) -> (Vec<Command>, &'static str) {
        let none = (Vec::new(), "edit");
        let clips = self.selected_clips(cx);
        let state = self.state.read(cx);
        let playhead = state.playhead;
        let tl = &state.bus.project().timeline;
        let mut commands = Vec::new();
        let mut label = "edit";
        match key {
            Key::PosX
            | Key::PosY
            | Key::Scale
            | Key::ScaleX
            | Key::ScaleY
            | Key::Rotation
            | Key::SkewX
            | Key::SkewY
            | Key::AnchorX
            | Key::AnchorY
            | Key::Opacity
            | Key::Crop(_)
            | Key::Pin(_) => {
                let Some(clip) = clips.first() else {
                    return none;
                };
                let property = property_of(key).unwrap();
                if clip.keyframes.iter().any(|k| k.property == property) {
                    commands.push(Command::SetKeyframe {
                        clip: clip.id,
                        property,
                        at: clip.start + clip_time(clip, playhead),
                        value: v,
                        ease: Default::default(),
                    });
                } else {
                    let mut transform = clip.transform.clone();
                    transform.set(property, v);
                    commands.push(Command::SetTransform {
                        clip: clip.id,
                        transform,
                    });
                }
            }
            Key::Mask(i, field) => {
                let Some(clip) = clips.first() else {
                    return none;
                };
                let Some(mask) = clip.masks.get(i) else {
                    return none;
                };
                let at = clip_time(clip, playhead);
                let mut now = mask.state_at(at);
                field.set(&mut now, v);
                let mut masks = clip.masks.clone();
                masks[i] = mask.with_state_at(at, now);
                commands.push(Command::SetMasks {
                    clip: clip.id,
                    masks,
                });
                label = "set_masks";
            }
            Key::TransitionDuration => {
                let Some((clip, edge)) = state.selected_transition else {
                    return none;
                };
                let Some((_, c)) = tl.clip(clip) else {
                    return none;
                };
                let current = match edge {
                    Edge::Start => c.transition_in.clone(),
                    Edge::End => c.transition_out.clone(),
                };
                let Some(mut transition) = current else {
                    return none;
                };
                transition.duration = Time::from_secs_f64(v as f64);
                label = "set_transition";
                commands.push(Command::SetTransition {
                    clip,
                    edge,
                    transition: Some(transition),
                });
            }
            Key::Exposure
            | Key::Contrast
            | Key::Saturation
            | Key::Gamma
            | Key::Temperature
            | Key::Tint
            | Key::Lift(_)
            | Key::GammaRgb(_)
            | Key::Gain(_) => {
                if state.workspace == Workspace::Color {
                    label = "set_timeline_grade";
                    let mut grade = tl.grade.clone();
                    color::set_grade_value(&mut grade, key, v);
                    commands.push(Command::SetTimelineGrade { grade });
                } else {
                    label = "set_grade";
                    for clip in clips.iter().filter(|c| is_media(c)) {
                        let mut grade = clip.grade.clone();
                        color::set_grade_value(&mut grade, key, v);
                        commands.push(Command::SetGrade {
                            clips: vec![clip.id],
                            grade,
                        });
                    }
                }
            }
            Key::FadeIn | Key::FadeOut => {
                label = "add_fade";
                let edge = if key == Key::FadeIn {
                    Edge::Start
                } else {
                    Edge::End
                };
                for clip in sound_clips(tl, &clips) {
                    commands.push(Command::AddFade {
                        clip: clip.id,
                        edge,
                        duration: Time::from_secs_f64(v as f64),
                    });
                }
            }
            Key::Volume(track) | Key::Pan(track) => {
                let Some(t) = tl.track(track) else {
                    return none;
                };
                let mut mix = t.mix.clone();
                if matches!(key, Key::Volume(_)) {
                    mix.volume_db = v;
                } else {
                    mix.pan = v;
                }
                commands.push(Command::SetTrackMix { track, mix });
            }
            Key::DuckReduction | Key::DuckAttack | Key::DuckRelease => {
                let Some(mut d) = tl.ducking.clone() else {
                    return none;
                };
                match key {
                    Key::DuckReduction => d.reduction_db = v,
                    Key::DuckAttack => d.attack = Time::from_secs_f64(v as f64),
                    _ => d.release = Time::from_secs_f64(v as f64),
                }
                commands.push(Command::SetDucking { ducking: Some(d) });
            }
            Key::Smoothness => {
                let Some(clip) = clips.first() else {
                    return none;
                };
                label = "set_stabilization";
                commands.push(Command::SetStabilization {
                    clips: vec![clip.id],
                    stabilization: koo_core::stabilize::Stabilization {
                        smoothness: v,
                        ..clip.stabilization.clone()
                    },
                });
            }
        }
        (commands, label)
    }

    fn set_ducking(&mut self, ducking: Option<Ducking>, cx: &mut Context<Self>) {
        self.run(vec![Command::SetDucking { ducking }], "set_ducking", cx);
    }
}

impl Render for InspectorPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.state.read(cx).workspace {
            Workspace::Color => self.timeline_color_view(window, cx),
            Workspace::Audio => self.timeline_audio_view(window, cx).into_any_element(),
            _ => self.edit_view(window, cx),
        };
        div()
            .id("inspector")
            .size_full()
            .overflow_y_scrollbar()
            .child(v_flex().p_3().gap_4().child(body))
    }
}

/// Group title above a block of controls.
pub(crate) fn section(title: &'static str, cx: &App) -> Div {
    v_flex().gap_0p5().child(
        div()
            .pb_1()
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .text_color(cx.theme().muted_foreground)
            .child(title),
    )
}

/// Group title with actions on the right.
pub(crate) fn section_with(title: &'static str, actions: impl IntoElement, cx: &App) -> Div {
    v_flex().gap_0p5().child(
        h_flex()
            .h(px(22.))
            .justify_between()
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .text_color(cx.theme().muted_foreground)
            .child(title)
            .child(actions),
    )
}

/// One muted line for an empty state.
fn empty(text: &'static str, cx: &App) -> AnyElement {
    div()
        .py_8()
        .flex()
        .justify_center()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text)
        .into_any_element()
}

/// The clips whose sound gain and fades change: the selected media clips,
/// video ones replaced by their linked audio (the renderer mixes audio
/// tracks only).
fn sound_clips<'a>(tl: &'a koo_core::model::Timeline, clips: &'a [Clip]) -> Vec<&'a Clip> {
    let mut out: Vec<&Clip> = Vec::new();
    for clip in clips.iter().filter(|c| is_media(c)) {
        let sound = clip::sound_clip(tl, clip);
        if !out.iter().any(|c| c.id == sound.id) {
            out.push(sound);
        }
    }
    out
}

fn is_media(clip: &Clip) -> bool {
    matches!(clip.content, ClipContent::Media { .. })
}

fn property_of(key: Key) -> Option<Property> {
    Some(match key {
        Key::PosX => Property::PositionX,
        Key::PosY => Property::PositionY,
        Key::Scale => Property::Scale,
        Key::ScaleX => Property::ScaleX,
        Key::ScaleY => Property::ScaleY,
        Key::Rotation => Property::Rotation,
        Key::SkewX => Property::SkewX,
        Key::SkewY => Property::SkewY,
        Key::AnchorX => Property::AnchorX,
        Key::AnchorY => Property::AnchorY,
        Key::Opacity => Property::Opacity,
        Key::Crop(0) => Property::CropLeft,
        Key::Crop(1) => Property::CropTop,
        Key::Crop(2) => Property::CropRight,
        Key::Crop(_) => Property::CropBottom,
        Key::Pin(i) => Property::PIN[i.min(7)],
        _ => return None,
    })
}

/// Playhead relative to the clip's start, clamped inside it.
fn clip_time(clip: &Clip, playhead: Time) -> Time {
    (playhead - clip.start).clamp(Time::ZERO, Time((clip.duration.0 - 1).max(0)))
}

fn audio_tracks(project: &Project) -> Vec<Track> {
    project
        .timeline
        .tracks
        .iter()
        .filter(|t| t.kind == koo_core::model::TrackKind::Audio)
        .cloned()
        .collect()
}
