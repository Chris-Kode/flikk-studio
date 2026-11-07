//! State behind the grade controls: what they grade, their sliders and a
//! drag in progress. It is keyed element state of the inspector, so the
//! panel needs no field for it. Drags show in the viewer uncommitted
//! (`EditorState::show_live`) and commit once, on release, as one command.

use super::fields::Field;
use crate::state::EditorState;
use crate::ui::kit::{NumberField, NumberFieldEvent, NumberFieldState, ScrubLabel, Slider, h_flex};
use flikk_core::ClipId;
use flikk_core::command::Command;
use flikk_core::model::Grade;
use gpui_kit::base::slider::{SliderEvent, SliderState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::HashMap;

/// What the controls grade.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Timeline,
    /// The first clip's grade is shown; edits apply field by field to each.
    Clips(Vec<ClipId>),
}

/// Where the controls sit: the Color workspace's own panel, in titled
/// sections, or a clip's Color tab, in plain groups.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Workspace,
    Tab,
}

impl Layout {
    /// Left inset of a row: past the section chevron in the workspace.
    pub fn inset(self) -> Pixels {
        px(match self {
            Layout::Workspace => 22.,
            Layout::Tab => 10.,
        })
    }

    fn label_width(self) -> Pixels {
        px(match self {
            Layout::Workspace => 78.,
            Layout::Tab => 64.,
        })
    }

    pub fn wheel_size(self) -> f32 {
        match self {
            Layout::Workspace => 60.,
            Layout::Tab => 54.,
        }
    }
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

/// A slider and the number field beside it, both showing one field.
struct Slot {
    slider: Entity<SliderState>,
    field: Entity<NumberFieldState>,
    /// The slider is held.
    dragging: bool,
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

    /// A drag or a typed value is under way.
    fn editing(&self, cx: &App) -> bool {
        self.curve_drag.is_some()
            || self.wheel_drag.is_some()
            || self
                .sliders
                .values()
                .any(|s| s.dragging || s.field.read(cx).is_editing())
    }

    /// Follow the project: called on every render. An edit under way keeps
    /// the target it began on, which it commits to, even when the target
    /// moves on meanwhile (the clip under the playhead, during playback).
    pub fn sync(&mut self, target: Target, base: Grade, cx: &mut App) {
        if self.editing(cx) {
            return;
        }
        // A preview left by an edit that committed nothing, or one of
        // another target, stops showing.
        if self.live.take().is_some() || self.target != target {
            self.state.update(cx, |s, cx| s.show_live(&[], cx));
        }
        self.target = target;
        self.base = base;
    }

    /// End a preview whose edit is over without a commit: a scrub
    /// released on its start value.
    fn end_stale_preview(&mut self, cx: &mut Context<Self>) {
        if self.live.is_some() && !self.editing(cx) {
            self.live = None;
            self.state.update(cx, |s, cx| s.show_live(&[], cx));
            cx.notify();
        }
    }

    /// Drop a curve or wheel drag whose release was missed (let go outside
    /// the window, or while the controls were not drawn), uncommitted.
    pub fn cancel_drag(&mut self, cx: &mut Context<Self>) {
        self.curve_drag = None;
        self.wheel_drag = None;
        self.end_stale_preview(cx);
        cx.notify();
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

    fn slot(&mut self, field: Field, window: &mut Window, cx: &mut Context<Self>) -> &Slot {
        if !self.sliders.contains_key(&field) {
            let spec = field.spec();
            let value = field.get(&self.base) * spec.factor;
            let slider = cx.new(|_| {
                SliderState::new()
                    .min(spec.min)
                    .max(spec.max)
                    .step(spec.step)
                    .default_value(value)
            });
            let number = cx.new(|cx| {
                let state = NumberFieldState::new(window, cx)
                    .min(spec.min as f64)
                    .max(spec.max as f64)
                    .step(spec.step as f64)
                    .decimals(spec.decimals)
                    .default_value(value as f64);
                if field.signed() {
                    state.signed()
                } else {
                    state
                }
            });
            let factor = spec.factor;
            let subs = vec![
                cx.subscribe(&slider, move |this, _, ev: &SliderEvent, cx| {
                    let (v, release) = match ev {
                        SliderEvent::Change(v) => (v.start(), false),
                        SliderEvent::Release(v) => (v.start(), true),
                    };
                    if let Some(s) = this.sliders.get_mut(&field) {
                        s.dragging = !release;
                    }
                    let v = v / factor;
                    if release {
                        this.commit(move |g| field.set(g, v), cx);
                    } else {
                        this.preview(move |g| field.set(g, v), cx);
                    }
                    cx.notify();
                }),
                cx.observe(&number, |this, _, cx| this.end_stale_preview(cx)),
                cx.subscribe(&number, move |this, _, ev: &NumberFieldEvent, cx| {
                    match *ev {
                        NumberFieldEvent::Change(v) => {
                            let v = v as f32 / factor;
                            this.preview(move |g| field.set(g, v), cx);
                        }
                        NumberFieldEvent::Commit(v) => {
                            let v = v as f32 / factor;
                            this.commit(move |g| field.set(g, v), cx);
                        }
                    }
                    cx.notify();
                }),
            ];
            self.sliders.insert(
                field,
                Slot {
                    slider,
                    field: number,
                    dragging: false,
                    _subscriptions: subs,
                },
            );
        }
        let display = field.get(self.shown()) * field.spec().factor;
        let slot = &self.sliders[&field];
        if !slot.dragging && (slot.slider.read(cx).value().start() - display).abs() > 1e-6 {
            slot.slider
                .update(cx, |s, cx| s.set_value(display, window, cx));
        }
        slot.field
            .update(cx, |f, cx| f.set_value(display as f64, cx));
        slot
    }

    /// Label (drag to scrub, double-click to reset), slider and number field.
    pub fn slider_row(
        &mut self,
        field: Field,
        layout: Layout,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let spec = field.spec();
        let this = cx.entity().downgrade();
        let slot = self.slot(field, window, cx);
        let label = ScrubLabel::new(&slot.field, spec.label).on_reset(move |_, cx| {
            let _ = this.update(cx, |ui, cx| {
                ui.commit(|g| field.set(g, spec.default), cx);
                cx.notify();
            });
        });
        h_flex()
            .flex_none()
            .h(px(24.))
            .gap(px(8.))
            .pl(layout.inset())
            .pr(px(10.))
            .child(
                div()
                    .w(layout.label_width())
                    .flex_none()
                    .min_w_0()
                    .overflow_hidden()
                    .child(label),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Slider::new(&slot.slider).when(field.centred(), |s| s.bipolar())),
            )
            .child(
                div()
                    .w(px(54.))
                    .flex_none()
                    .child(NumberField::new(&slot.field).h(px(20.)).text_size(px(10.))),
            )
    }
}
