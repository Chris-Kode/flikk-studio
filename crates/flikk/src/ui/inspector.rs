//! Inspector, contextual by workspace: in Edit, the selected transition, or
//! the selected clip's properties by kind with its effects, masks, grade
//! and linked audio; in Color and Audio, the whole timeline's grade, or its
//! loudness and ducking. Properties sit in collapsible groups of 26 px rows:
//! a label that scrubs, the value, and the keyframe lane.

mod audio;
mod clip;
mod color;
mod controls;
mod edit;
mod effects;
mod generator;
mod masks;
mod param;
mod speed;
mod stabilize;
mod tone_map;
mod transition;

pub(crate) use clip::sound_clip;

use crate::state::EditorState;
use crate::ui::caption_style::{self, CaptionStylePanel};
use crate::ui::kit::theme as t;
use crate::ui::kit::{
    Button, EmptyState, IconName, NumberFieldEvent, NumberFieldState, Panel, PanelHeader,
    ScrollableElement as _, SectionHeader, h_flex, v_flex,
};
use clip::PictureTab;
use flikk_core::MaskId;
use flikk_core::agent::Workspace;
use flikk_core::analysis::Loudness;
use flikk_core::command::{Command, Edge};
use flikk_core::model::{Clip, ClipContent, Ducking, Project, Property, Track};
use flikk_core::{ClipId, Time};
use gpui_kit::base::input::{InputEvent, InputState, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
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
    lufs_target: Entity<NumberFieldState>,
    peak_target: Entity<NumberFieldState>,
    picture_tab: PictureTab,
    /// Groups folded to their header, by key.
    collapsed: HashSet<&'static str>,
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
    collapsed_fx: HashSet<flikk_core::EffectId>,
    /// The mask whose settings show in the Masks tab.
    open_mask: Option<MaskId>,
    /// Clip name field while renaming, and the clip it renames.
    name_field: Entity<InputState>,
    renaming: Option<ClipId>,
    /// Speed in percent, the clip it shows, and whether a change ripples.
    speed_field: Entity<NumberFieldState>,
    speed_clip: Option<ClipId>,
    speed_ripple: bool,
    _subscriptions: Vec<Subscription>,
}

/// What the panel shows for a selection: its header, page tabs and body.
pub(super) struct View {
    header: Option<PanelHeader>,
    tabs: Option<AnyElement>,
    body: AnyElement,
}

impl View {
    fn new(header: PanelHeader, body: impl IntoElement) -> Self {
        Self {
            header: Some(header),
            tabs: None,
            body: body.into_any_element(),
        }
    }
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
        let end_field = cx.new(|cx| InputState::new(window, cx));
        let speed_field = cx.new(|cx| {
            NumberFieldState::new(window, cx)
                .min(flikk_core::speed::MIN_SPEED * 100.)
                .max(flikk_core::speed::MAX_SPEED * 100.)
                .default_value(100.)
        });
        let commit = |ev: &InputEvent| {
            matches!(
                ev,
                InputEvent::PressEnter { shift: false, .. } | InputEvent::Blur
            )
        };
        let presets = state.read(cx).presets.clone();
        let target = |value: f32, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| {
                NumberFieldState::new(window, cx)
                    .min(-60.)
                    .max(0.)
                    .step(0.1)
                    .decimals(1)
                    .default_value(value as f64)
            })
        };
        let lufs_target = target(presets.loudness_lufs, window, cx);
        let peak_target = target(presets.true_peak_db, window, cx);
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
            cx.subscribe(&speed_field, |this, _, ev: &NumberFieldEvent, cx| {
                if let NumberFieldEvent::Commit(percent) = *ev {
                    this.commit_speed(percent / 100., cx);
                }
            }),
        ];
        let caption_style = cx.new(|cx| {
            CaptionStylePanel::new(state.clone(), caption_style::Target::Active, window, cx)
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
                _ if crate::demo::speed::enabled() => PictureTab::Speed,
                _ => PictureTab::default(),
            },
            collapsed: HashSet::from(["crop", "pin"]),
            text,
            text_clip: None,
            start_field,
            end_field,
            timing_clip: None,
            caption_style,
            fx_controls: HashMap::new(),
            collapsed_fx: HashSet::new(),
            open_mask: None,
            name_field,
            renaming: None,
            speed_field,
            speed_clip: None,
            speed_ripple: false,
            _subscriptions,
        }
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

    /// Apply a field's new model value.
    fn commit(&mut self, key: Key, v: f32, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| s.show_live(&[], cx));
        let (commands, label) = self.commands(key, v, cx);
        self.run(commands, label, cx);
    }

    /// Show a value in the viewer while its label is scrubbed.
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
                    stabilization: flikk_core::stabilize::Stabilization {
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

    fn is_open(&self, group: &'static str) -> bool {
        !self.collapsed.contains(group)
    }

    /// The header of the collapsible group `key`; a click folds or unfolds
    /// it.
    fn header(
        &self,
        key: &'static str,
        title: impl Into<SharedString>,
        cx: &Context<Self>,
    ) -> SectionHeader {
        let panel = cx.entity().downgrade();
        SectionHeader::new(key, title)
            .open(self.is_open(key))
            .on_toggle(move |open, _, cx| {
                let _ = panel.update(cx, |this, cx| {
                    if *open {
                        this.collapsed.remove(key);
                    } else {
                        this.collapsed.insert(key);
                    }
                    cx.notify();
                });
            })
    }
}

impl Render for InspectorPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = match self.state.read(cx).workspace {
            Workspace::Color => self.timeline_color_view(window, cx),
            Workspace::Audio => self.timeline_audio_view(window, cx),
            _ => self.edit_view(window, cx),
        };
        Panel::new()
            .children(view.header)
            .children(view.tabs)
            .child(
                div()
                    .id("inspector")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .child(v_flex().pb(px(12.)).child(view.body)),
            )
    }
}

/// The panel header: "Inspector" and what it shows.
fn panel_header(context: impl Into<SharedString>) -> PanelHeader {
    PanelHeader::new().title("Inspector").context(context)
}

/// A group of rows under its header, ruled off below. Pass no rows for a
/// folded group.
fn group<E: IntoElement>(header: SectionHeader, rows: impl IntoIterator<Item = E>) -> Div {
    let rows: Vec<AnyElement> = rows
        .into_iter()
        .map(IntoElement::into_any_element)
        .collect();
    v_flex()
        .flex_none()
        .border_b_1()
        .border_color(t::LN)
        .child(header)
        .when(!rows.is_empty(), |g| {
            g.child(v_flex().pb(px(6.)).children(rows))
        })
}

/// An inspector row: the 76 px label, the control, and the 54 px keyframe
/// lane, kept empty where there is none so the fields line up.
fn row(label: impl IntoElement, control: impl IntoElement, lane: Option<AnyElement>) -> Div {
    h_flex()
        .flex_none()
        .h(px(26.))
        .gap(px(6.))
        .pl(px(22.))
        .pr(px(10.))
        .child(
            div()
                .w(px(76.))
                .flex_none()
                .min_w_0()
                .overflow_hidden()
                .child(label),
        )
        .child(div().flex_1().min_w_0().child(control))
        .child(h_flex().w(px(54.)).flex_none().justify_end().children(lane))
}

/// A row's label that does not scrub.
fn label(text: impl Into<SharedString>) -> Div {
    div()
        .min_w_0()
        .truncate()
        .text_size(px(12.))
        .text_color(t::T2)
        .child(text.into())
}

/// A control spanning a group's width, inset like the rows.
fn wide(child: impl IntoElement) -> Div {
    div()
        .flex_none()
        .pl(px(22.))
        .pr(px(10.))
        .py(px(3.))
        .child(child)
}

/// A select-like field: the current value and a chevron; give it a
/// dropdown menu of the choices.
fn select_field(id: impl Into<ElementId>, value: impl Into<SharedString>) -> Button {
    Button::new(id)
        .small()
        .label(value)
        .trailing_icon(IconName::ChevronDown)
        .w_full()
        .justify_between()
        .overflow_hidden()
        .px(px(6.))
        .rounded(t::RADIUS_SM)
        .bg(t::G)
        .font_weight(FontWeight::NORMAL)
}

/// A one-line error under a group's rows.
fn error_line(message: impl Into<SharedString>) -> Div {
    div()
        .pl(px(22.))
        .pr(px(10.))
        .py(px(2.))
        .text_size(px(11.))
        .text_color(t::ERR)
        .child(message.into())
}

/// The empty state of a view with nothing to show.
fn empty(text: &'static str, _: &App) -> AnyElement {
    EmptyState::new(IconName::MousePointerClick, text)
        .compact()
        .into_any_element()
}

/// The clips whose sound gain and fades change: the selected media clips,
/// video ones replaced by their linked audio (the renderer mixes audio
/// tracks only).
fn sound_clips<'a>(tl: &'a flikk_core::model::Timeline, clips: &'a [Clip]) -> Vec<&'a Clip> {
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
        .filter(|t| t.kind == flikk_core::model::TrackKind::Audio)
        .cloned()
        .collect()
}
