//! Inspector, contextual by workspace and selection: transform and keyframes
//! (Edit), primary grade (Color), gain, fades, mixer, loudness and ducking
//! (Audio).

mod audio;
mod color;
mod edit;
mod param;

use crate::state::EditorState;
use gpui_kit::component::input::InputState;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::*;
use koo_core::agent::Workspace;
use koo_core::analysis::Loudness;
use koo_core::command::{Command, Edge};
use koo_core::model::{Clip, ClipContent, Ducking, Project, Property, Track};
use koo_core::{ClipId, Time};
use param::{Key, Param};
use std::collections::HashMap;

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
    _observe: Subscription,
}

impl InspectorPanel {
    pub fn new(state: Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let _observe = cx.observe(&state, |_, _, cx| cx.notify());
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
            _observe,
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

    fn primary(&self, cx: &App) -> Option<Clip> {
        self.selected_clips(cx).into_iter().next()
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
            Key::PosX | Key::PosY | Key::Scale | Key::Rotation | Key::Opacity => {
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
                    match key {
                        Key::PosX => transform.position[0] = v,
                        Key::PosY => transform.position[1] = v,
                        Key::Scale => transform.scale = v,
                        Key::Rotation => transform.rotation_deg = v,
                        _ => transform.opacity = v,
                    }
                    commands.push(Command::SetTransform {
                        clip: clip.id,
                        transform,
                    });
                }
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
                label = "set_grade";
                for clip in clips.iter().filter(|c| is_media(c)) {
                    let mut grade = clip.grade.clone();
                    match key {
                        Key::Exposure => grade.exposure = v,
                        Key::Contrast => grade.contrast = v,
                        Key::Saturation => grade.saturation = v,
                        Key::Gamma => grade.gamma = v,
                        Key::Temperature => grade.temperature = v,
                        Key::Tint => grade.tint = v,
                        Key::Lift(i) => grade.lift[i] = v,
                        Key::GammaRgb(i) => grade.gamma_rgb[i] = v,
                        Key::Gain(i) => grade.gain[i] = v,
                        _ => {}
                    }
                    commands.push(Command::SetGrade {
                        clips: vec![clip.id],
                        grade,
                    });
                }
            }
            Key::ClipGain => {
                label = "set_gain";
                for clip in clips.iter().filter(|c| is_media(c)) {
                    commands.push(Command::SetGain {
                        clip: clip.id,
                        db: v,
                    });
                }
            }
            Key::FadeIn | Key::FadeOut => {
                label = "add_fade";
                let edge = if key == Key::FadeIn {
                    Edge::Start
                } else {
                    Edge::End
                };
                for clip in clips.iter().filter(|c| is_media(c)) {
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
            Workspace::Color => self.color_view(window, cx),
            Workspace::Audio => self.audio_view(window, cx),
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
fn section(title: &'static str, cx: &App) -> Div {
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
fn section_with(title: &'static str, actions: impl IntoElement, cx: &App) -> Div {
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

fn is_media(clip: &Clip) -> bool {
    matches!(clip.content, ClipContent::Media { .. })
}

fn property_of(key: Key) -> Option<Property> {
    Some(match key {
        Key::PosX => Property::PositionX,
        Key::PosY => Property::PositionY,
        Key::Scale => Property::Scale,
        Key::Rotation => Property::Rotation,
        Key::Opacity => Property::Opacity,
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
