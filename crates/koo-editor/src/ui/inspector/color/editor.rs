//! State behind the grade controls: what they grade, their sliders and a
//! drag in progress. It is keyed element state of the inspector, so the
//! panel needs no field for it. Drags show in the viewer uncommitted
//! (`EditorState::show_live`) and commit once, on release, as one command.

use super::super::param::format_value;
use super::fields::Field;
use crate::state::EditorState;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex};
use gpui_kit::*;
use koo_core::ClipId;
use koo_core::command::Command;
use koo_core::model::Grade;
use std::collections::HashMap;

/// What the controls grade.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Timeline,
    /// The first clip's grade is shown; edits apply field by field to each.
    Clips(Vec<ClipId>),
}

/// A curve point being dragged.
pub struct CurveDrag {
    pub channel: usize,
    /// The channel's points, end points included, sorted.
    pub points: Vec<[f32; 2]>,
    pub index: usize,
    /// Dragged out of the editor: removed on release.
    pub out: bool,
}

pub struct GradeUi {
    state: Entity<EditorState>,
    pub target: Target,
    /// The committed grade of the target.
    pub base: Grade,
    /// What a drag in progress shows.
    pub live: Option<Grade>,
    sliders: HashMap<Field, Slot>,
    /// Curves channel on screen: 0 master, then red, green, blue.
    pub channel: usize,
    pub curve_drag: Option<CurveDrag>,
    /// Wheel index and whether its master bar (not the puck) is dragged.
    pub wheel_drag: Option<(usize, bool)>,
    /// Color workspace: the timeline grade rather than the clip's.
    pub timeline_tab: bool,
    pub matching: bool,
}

struct Slot {
    slider: Entity<SliderState>,
    field: Entity<InputState>,
    value: f32,
    dragging: bool,
    typing: bool,
    _subscriptions: Vec<Subscription>,
}

impl GradeUi {
    pub fn new(state: Entity<EditorState>) -> Self {
        Self {
            state,
            target: Target::Timeline,
            base: Grade::default(),
            live: None,
            sliders: HashMap::new(),
            channel: 0,
            curve_drag: None,
            wheel_drag: None,
            timeline_tab: false,
            matching: false,
        }
    }

    /// The grade on screen: the drag's, else the committed one.
    pub fn shown(&self) -> &Grade {
        self.live.as_ref().unwrap_or(&self.base)
    }

    pub fn dragging(&self) -> bool {
        self.curve_drag.is_some()
            || self.wheel_drag.is_some()
            || self.sliders.values().any(|s| s.dragging)
    }

    /// Follow the project: called on every render.
    pub fn sync(&mut self, target: Target, base: Grade) {
        if target != self.target {
            self.curve_drag = None;
            self.wheel_drag = None;
            self.live = None;
        }
        self.target = target;
        self.base = base;
        if !self.dragging() {
            self.live = None;
        }
    }

    /// Commands that apply `edit` to the target's grade(s).
    fn commands(&self, edit: &dyn Fn(&mut Grade), cx: &App) -> (Vec<Command>, &'static str) {
        let project = self.state.read(cx).bus.project();
        match &self.target {
            Target::Timeline => {
                let mut grade = project.timeline.grade.clone();
                edit(&mut grade);
                (
                    vec![Command::SetTimelineGrade { grade }],
                    "set_timeline_grade",
                )
            }
            Target::Clips(ids) => {
                let commands = ids
                    .iter()
                    .filter_map(|id| project.timeline.clip(*id))
                    .map(|(_, clip)| {
                        let mut grade = clip.grade.clone();
                        edit(&mut grade);
                        Command::SetGrade {
                            clips: vec![clip.id],
                            grade,
                        }
                    })
                    .collect();
                (commands, "set_grade")
            }
        }
    }

    /// Show `edit` in the viewer without committing it.
    pub fn preview(&mut self, edit: impl Fn(&mut Grade), cx: &mut App) {
        let mut shown = self.base.clone();
        edit(&mut shown);
        self.live = Some(shown);
        let (commands, _) = self.commands(&edit, cx);
        self.state.update(cx, |s, cx| s.show_live(&commands, cx));
    }

    /// Commit `edit` as one undo step.
    pub fn commit(&mut self, edit: impl Fn(&mut Grade), cx: &mut App) {
        self.live = None;
        let (commands, label) = self.commands(&edit, cx);
        self.state.update(cx, |s, cx| {
            s.show_live(&[], cx);
            match commands.len() {
                0 => {}
                1 => {
                    s.execute(commands.into_iter().next().unwrap(), cx);
                }
                _ => {
                    s.execute_batch(commands, label, cx);
                }
            }
        });
    }

    /// Replace the whole grade (reset, match).
    pub fn commit_grade(&mut self, grade: Grade, cx: &mut App) {
        self.commit(move |g| *g = grade.clone(), cx);
    }

    // ------------------------------------------------------------- sliders

    fn slot(&mut self, field: Field, window: &mut Window, cx: &mut Context<Self>) -> &mut Slot {
        if !self.sliders.contains_key(&field) {
            let spec = field.spec();
            let value = field.get(&self.base);
            let slider = cx.new(|_| {
                SliderState::new()
                    .min(spec.min)
                    .max(spec.max)
                    .step(spec.step)
                    .default_value(value * spec.factor)
            });
            let text = format_value(value * spec.factor, spec.decimals);
            let input = cx.new(|cx| InputState::new(window, cx).default_value(text));
            let subs = vec![
                cx.subscribe_in(
                    &slider,
                    window,
                    move |this, _, ev: &SliderEvent, window, cx| {
                        let (v, release) = match ev {
                            SliderEvent::Change(v) => (v.start(), false),
                            SliderEvent::Release(v) => (v.start(), true),
                        };
                        if let Some(s) = this.sliders.get_mut(&field) {
                            s.dragging = !release;
                            let text = format_value(v, spec.decimals);
                            s.field.update(cx, |f, cx| f.set_value(text, window, cx));
                        }
                        let v = v / spec.factor;
                        if release {
                            this.commit(move |g| field.set(g, v), cx);
                        } else {
                            this.preview(move |g| field.set(g, v), cx);
                        }
                    },
                ),
                cx.subscribe(&input, move |this, input, ev: &InputEvent, cx| {
                    if let Some(s) = this.sliders.get_mut(&field) {
                        s.typing = matches!(ev, InputEvent::Change);
                    }
                    if !matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                        return;
                    }
                    let Ok(v) = input.read(cx).value().trim().parse::<f32>() else {
                        return;
                    };
                    let v = v.clamp(spec.min, spec.max) / spec.factor;
                    if (v - field.get(&this.base)).abs() > f32::EPSILON {
                        if let Some(s) = this.sliders.get_mut(&field) {
                            s.value = f32::NAN;
                        }
                        this.commit(move |g| field.set(g, v), cx);
                    }
                }),
            ];
            self.sliders.insert(
                field,
                Slot {
                    slider,
                    field: input,
                    value,
                    dragging: false,
                    typing: false,
                    _subscriptions: subs,
                },
            );
        }
        let value = field.get(self.shown());
        let slot = self.sliders.get_mut(&field).unwrap();
        if !slot.dragging && (value - slot.value).abs() > 1e-6 {
            slot.value = value;
            let spec = field.spec();
            let display = value * spec.factor;
            slot.slider
                .update(cx, |s, cx| s.set_value(display, window, cx));
            if !slot.typing {
                let text = format_value(display, spec.decimals);
                slot.field.update(cx, |f, cx| f.set_value(text, window, cx));
            }
        }
        slot
    }

    /// Label (double-click resets), slider and number field on a 28 px row.
    pub fn slider_row(&mut self, field: Field, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let spec = field.spec();
        let muted = cx.theme().muted_foreground;
        let this = cx.entity();
        let slot = self.slot(field, window, cx);
        h_flex()
            .h(px(28.))
            .gap_2()
            .child(
                div()
                    .id(SharedString::from(format!("grade-label-{field:?}")))
                    .w(px(64.))
                    .flex_none()
                    .text_xs()
                    .text_color(muted)
                    .truncate()
                    .child(spec.label)
                    .on_click(move |ev: &ClickEvent, _, cx| {
                        if ev.click_count() >= 2 {
                            this.update(cx, |ui, cx| ui.commit(|g| field.set(g, spec.default), cx));
                        }
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Slider::new(&slot.slider).w_full()),
            )
            .child(
                div()
                    .w(px(56.))
                    .flex_none()
                    .child(Input::new(&slot.field).xsmall()),
            )
    }
}
