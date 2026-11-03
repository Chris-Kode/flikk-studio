//! The numeric scrub field of inspector rows: a 22 px mono field with an
//! optional unit, a label that scrubs the value on a horizontal drag, and
//! the keyframe diamond with its previous / next arrows.
//!
//! [`NumberFieldState`] owns the value and both ways of changing it. A scrub
//! emits [`NumberFieldEvent::Change`] for every step, to preview, and one
//! [`NumberFieldEvent::Commit`] on release; typing commits on Enter or blur.
//! The model is followed with [`NumberFieldState::set_value`], which leaves
//! a scrub or an edit in progress alone.

use super::{icon::Icon, icon::IconName, input::project_style, theme as t, tooltip::Tooltip};
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::base::{Input as BaseInput, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NumberFieldEvent {
    /// The value while a scrub runs: show it, don't commit it.
    Change(f64),
    /// A scrub released or a typed value entered.
    Commit(f64),
}

#[derive(Clone, Copy)]
struct Scrub {
    from: f64,
    x: Pixels,
    moved: bool,
}

pub struct NumberFieldState {
    input: Entity<InputState>,
    value: f64,
    min: Option<f64>,
    max: Option<f64>,
    step: f64,
    decimals: usize,
    signed: bool,
    scrub: Option<Scrub>,
    /// The field holds typed text not yet entered.
    edited: bool,
    _subscription: Subscription,
}

impl EventEmitter<NumberFieldEvent> for NumberFieldState {}

impl NumberFieldState {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx));
        let _subscription = cx.subscribe_in(
            &input,
            window,
            |this, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::Change => this.edited = true,
                InputEvent::PressEnter { .. } | InputEvent::Blur => this.submit(window, cx),
                InputEvent::Focus => {}
            },
        );
        Self {
            input,
            value: 0.,
            min: None,
            max: None,
            step: 1.,
            decimals: 0,
            signed: false,
            scrub: None,
            edited: false,
            _subscription,
        }
    }

    pub fn min(mut self, min: f64) -> Self {
        self.min = Some(min);
        self
    }

    pub fn max(mut self, max: f64) -> Self {
        self.max = Some(max);
        self
    }

    /// What a scrub and a typed value round to.
    pub fn step(mut self, step: f64) -> Self {
        self.step = step;
        self
    }

    pub fn decimals(mut self, decimals: usize) -> Self {
        self.decimals = decimals;
        self
    }

    /// Show `+` on positive values: offsets such as exposure or tint.
    pub fn signed(mut self) -> Self {
        self.signed = true;
        self
    }

    pub fn default_value(mut self, value: f64) -> Self {
        self.value = value;
        self
    }

    pub fn value(&self) -> f64 {
        self.value
    }

    /// The label is being dragged and the value has moved.
    pub fn is_scrubbing(&self) -> bool {
        self.scrub.is_some_and(|s| s.moved)
    }

    /// Follow the model, except while the user scrubs. A field being typed
    /// in keeps its text until Enter or blur.
    pub fn set_value(&mut self, value: f64, cx: &mut Context<Self>) {
        if self.scrub.is_none() && value != self.value {
            self.value = value;
            cx.notify();
        }
    }

    fn clamp(&self, v: f64) -> f64 {
        let v = self.min.map_or(v, |min| v.max(min));
        self.max.map_or(v, |max| v.min(max))
    }

    fn text(&self, v: f64) -> String {
        let d = self.decimals;
        let s = format!("{v:.d$}");
        if s.trim_start_matches('-')
            .chars()
            .all(|c| c == '0' || c == '.')
        {
            s.trim_start_matches('-').to_owned()
        } else if let Some(abs) = s.strip_prefix('-') {
            format!("\u{2212}{abs}")
        } else if self.signed {
            format!("+{s}")
        } else {
            s
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.edited) {
            return;
        }
        let typed = self.input.read(cx).value();
        let typed = typed.trim().replace('\u{2212}', "-");
        if let Ok(v) = typed.trim_start_matches('+').parse::<f64>() {
            let v = self.clamp(v);
            if (v - self.value).abs() > 1e-9 {
                self.value = v;
                cx.emit(NumberFieldEvent::Commit(v));
            }
        }
        let text = self.text(self.value);
        self.input.update(cx, |i, cx| i.set_value(text, window, cx));
        cx.notify();
    }

    fn begin_scrub(&mut self, x: Pixels) {
        self.scrub = Some(Scrub {
            from: self.value,
            x,
            moved: false,
        });
    }

    fn scrub_to(&mut self, x: Pixels, fine: bool, cx: &mut Context<Self>) {
        let Some(scrub) = &mut self.scrub else {
            return;
        };
        scrub.moved = true;
        let per_px = match (self.min, self.max) {
            (Some(min), Some(max)) => ((max - min) / 400.).max(self.step),
            _ => self.step,
        };
        let per_px = if fine { per_px * 0.1 } else { per_px };
        let raw = scrub.from + f64::from(x - scrub.x) * per_px;
        let v = self.clamp((raw / self.step).round() * self.step);
        if v != self.value {
            self.value = v;
            cx.emit(NumberFieldEvent::Change(v));
        }
        cx.notify();
    }

    fn end_scrub(&mut self, cx: &mut Context<Self>) {
        let Some(scrub) = self.scrub.take() else {
            return;
        };
        if (self.value - scrub.from).abs() > 1e-9 {
            cx.emit(NumberFieldEvent::Commit(self.value));
        }
        cx.notify();
    }

    /// How far the running scrub has moved the value, as shown.
    fn delta(&self) -> Option<String> {
        let scrub = self.scrub.filter(|s| s.moved)?;
        let d = self.value - scrub.from;
        let text = self.text(d.abs());
        Some(if d < 0. {
            format!("\u{2212}{text}")
        } else {
            format!("+{}", text.trim_start_matches('+'))
        })
    }

    /// Where the value sits in its range, for the scrub fill.
    fn fraction(&self) -> Option<f32> {
        let (min, max) = (self.min?, self.max?);
        (max > min).then(|| ((self.value - min) / (max - min)).clamp(0., 1.) as f32)
    }
}

/// The field: the value in mono with an optional prefix and unit. Click to
/// type a value.
#[derive(IntoElement)]
pub struct NumberField {
    state: Entity<NumberFieldState>,
    prefix: Option<SharedString>,
    unit: Option<SharedString>,
    mixed: bool,
    keyed: bool,
    disabled: bool,
    style: StyleRefinement,
}

impl NumberField {
    pub fn new(state: &Entity<NumberFieldState>) -> Self {
        Self {
            state: state.clone(),
            prefix: None,
            unit: None,
            mixed: false,
            keyed: false,
            disabled: false,
            style: StyleRefinement::default(),
        }
    }

    /// A short name inside the field, before the value: `X`, `W`, `Pad`.
    pub fn prefix(mut self, prefix: impl Into<SharedString>) -> Self {
        self.prefix = Some(prefix.into());
        self
    }

    pub fn unit(mut self, unit: impl Into<SharedString>) -> Self {
        self.unit = Some(unit.into());
        self
    }

    /// The selected clips disagree: show `—` until a value is typed.
    pub fn mixed(mut self, mixed: bool) -> Self {
        self.mixed = mixed;
        self
    }

    /// There is a keyframe at the playhead.
    pub fn keyed(mut self, keyed: bool) -> Self {
        self.keyed = keyed;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl Styled for NumberField {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for NumberField {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let s = self.state.read(cx);
        let input = s.input.clone();
        let scrubbing = s.is_scrubbing();
        let fraction = s.fraction().filter(|_| scrubbing);
        let shown = if self.mixed && !scrubbing {
            String::new()
        } else {
            s.text(s.value)
        };
        let focused = input.read(cx).has_selection_focus(window, cx);
        if !s.edited && input.read(cx).value().as_ref() != shown {
            input.update(cx, |i, cx| i.set_value(shown, window, cx));
        }
        project_style(&input, cx);
        input.update(cx, |i, cx| i.set_disabled(self.disabled, cx));

        let (border, ring) = if scrubbing || focused {
            (t::AC, Some(t::field_ring(t::AC)))
        } else if self.keyed {
            (t::alpha(t::AC, 0.4), None)
        } else {
            (t::LN, None)
        };
        let idle = !scrubbing && !focused && !self.keyed && !self.disabled;
        let mut field = div()
            .id(("flikk-number", self.state.entity_id()))
            .relative()
            .flex()
            .items_center()
            .w_full()
            .h(px(22.))
            .px(px(6.))
            .overflow_hidden()
            .rounded(t::RADIUS_SM)
            .border_1()
            .border_color(border)
            .bg(t::G)
            .font_family(t::mono())
            .text_size(px(11.))
            .line_height(px(16.))
            .text_color(t::T1)
            .when_some(ring, |f, ring| f.shadow(ring))
            .when(idle, |f| f.hover(|s| s.border_color(t::LN_HV)))
            .when(self.disabled, |f| f.opacity(0.4))
            .when(!self.disabled, |f| {
                let input = input.clone();
                f.cursor_text()
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        if !input.read(cx).has_selection_focus(window, cx) {
                            input.update(cx, |i, cx| i.focus(window, cx));
                        }
                    })
            });
        field.style().refine(&self.style);

        field
            .children(fraction.map(|f| {
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(relative(f))
                    .bg(t::alpha(t::AC, 0.133))
                    .border_r_1()
                    .border_color(t::AC)
            }))
            .children(
                self.prefix
                    .map(|p| div().flex_none().mr(px(4.)).text_color(t::T3).child(p)),
            )
            .map(|f| {
                if self.mixed && !focused && !scrubbing {
                    f.child(div().flex_1().text_color(t::T3).child("\u{2014}"))
                } else {
                    f.child(div().flex_1().min_w_0().child(BaseInput::new(&input)))
                }
            })
            .children(
                self.unit
                    .map(|u| div().flex_none().pl(px(4.)).text_color(t::T3).child(u)),
            )
    }
}

#[derive(Clone)]
struct ScrubDrag(EntityId);

impl Render for ScrubDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

type Handler = Rc<dyn Fn(&mut Window, &mut App)>;

/// A row label that scrubs its field: drag left or right to change the
/// value, Shift for fine steps, double-click to reset.
#[derive(IntoElement)]
pub struct ScrubLabel {
    state: Entity<NumberFieldState>,
    label: SharedString,
    keyed: bool,
    disabled: bool,
    on_reset: Option<Handler>,
}

impl ScrubLabel {
    pub fn new(state: &Entity<NumberFieldState>, label: impl Into<SharedString>) -> Self {
        Self {
            state: state.clone(),
            label: label.into(),
            keyed: false,
            disabled: false,
            on_reset: None,
        }
    }

    /// There is a keyframe at the playhead.
    pub fn keyed(mut self, keyed: bool) -> Self {
        self.keyed = keyed;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Double-click: back to the default.
    pub fn on_reset(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_reset = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for ScrubLabel {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let id = self.state.entity_id();
        let group = SharedString::from(format!("flikk-scrub-{id}"));
        let delta = self.state.read(cx).delta();
        let scrubbing = delta.is_some();
        let color = if scrubbing {
            t::AC
        } else if self.keyed {
            t::T1
        } else {
            t::T2
        };
        let underline = if scrubbing {
            t::AC
        } else {
            t::alpha(t::ON_AC, 0.19)
        };

        h_flex()
            .id(("flikk-scrub", id))
            .group(group.clone())
            .gap(px(4.))
            .min_w_0()
            .text_size(px(12.))
            .text_color(color)
            .when(self.disabled, |l| l.opacity(0.4))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_decoration_1()
                    .text_decoration_color(underline)
                    .when(!scrubbing && !self.disabled, |d| {
                        d.group_hover(group, |s| s.text_decoration_color(t::alpha(t::ON_AC, 0.44)))
                    })
                    .child(self.label),
            )
            .children(delta.map(|d| {
                div()
                    .flex_none()
                    .font_family(t::mono())
                    .text_size(px(10.))
                    .child(d)
            }))
            .when(!self.disabled, |l| {
                l.cursor(CursorStyle::ResizeLeftRight)
                    .when(!scrubbing, |l| l.hover(|s| s.text_color(t::T1)))
                    .on_mouse_down(
                        MouseButton::Left,
                        window.listener_for(&self.state, |s, e: &MouseDownEvent, _, _| {
                            s.begin_scrub(e.position.x)
                        }),
                    )
                    .on_drag(ScrubDrag(id), |drag, _, _, cx| {
                        cx.stop_propagation();
                        cx.new(|_| drag.clone())
                    })
                    .on_drag_move(window.listener_for(
                        &self.state,
                        move |s, e: &DragMoveEvent<ScrubDrag>, _, cx| {
                            if e.drag(cx).0 == id {
                                s.scrub_to(e.event.position.x, e.event.modifiers.shift, cx);
                            }
                        },
                    ))
                    .on_mouse_up(
                        MouseButton::Left,
                        window.listener_for(&self.state, |s, _, _, cx| s.end_scrub(cx)),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        window.listener_for(&self.state, |s, _, _, cx| s.end_scrub(cx)),
                    )
                    .when_some(self.on_reset, |l, reset| {
                        l.on_click(move |e, window, cx| {
                            if e.click_count() >= 2 {
                                reset(window, cx);
                            }
                        })
                    })
            })
    }
}

/// The keyframe diamond's state for a parameter at the playhead.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Keyframe {
    /// Not animated.
    #[default]
    None,
    /// Animated, with no keyframe here.
    Off,
    /// A keyframe at the playhead.
    On,
    /// The selected clips disagree.
    Mixed,
}

/// Previous keyframe, the diamond that adds or removes one at the playhead,
/// next keyframe. An arrow without a handler has nowhere to go.
#[derive(IntoElement)]
pub struct KeyframeNav {
    id: ElementId,
    keyframe: Keyframe,
    disabled: bool,
    on_toggle: Option<Handler>,
    on_prev: Option<Handler>,
    on_next: Option<Handler>,
}

impl KeyframeNav {
    pub fn new(id: impl Into<ElementId>, keyframe: Keyframe) -> Self {
        Self {
            id: id.into(),
            keyframe,
            disabled: false,
            on_toggle: None,
            on_prev: None,
            on_next: None,
        }
    }

    /// The playhead is outside the clip: keyframes can't be set here.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_toggle(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Rc::new(handler));
        self
    }

    pub fn on_prev(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_prev = Some(Rc::new(handler));
        self
    }

    pub fn on_next(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_next = Some(Rc::new(handler));
        self
    }
}

/// The diamond, painted: outline, filled, or outline with a bar.
fn diamond(keyframe: Keyframe, color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let c = bounds.center();
            let rhombus = |r: f32, mut b: PathBuilder| {
                b.move_to(point(c.x, c.y - px(r)));
                b.line_to(point(c.x + px(r), c.y));
                b.line_to(point(c.x, c.y + px(r)));
                b.line_to(point(c.x - px(r), c.y));
                b.close();
                b
            };
            let builder = if keyframe == Keyframe::On {
                rhombus(5., PathBuilder::fill())
            } else {
                rhombus(4.5, PathBuilder::stroke(px(1.)))
            };
            if let Ok(path) = builder.build() {
                window.paint_path(path, color);
            }
            if keyframe == Keyframe::Mixed {
                let mut bar = PathBuilder::stroke(px(1.));
                bar.move_to(point(c.x - px(2.), c.y));
                bar.line_to(point(c.x + px(2.), c.y));
                if let Ok(path) = bar.build() {
                    window.paint_path(path, color);
                }
            }
        },
    )
    .size(px(11.))
}

impl RenderOnce for KeyframeNav {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let (arrows, mark): (Rgba, Rgba) = match self.keyframe {
            Keyframe::None | Keyframe::Mixed => (t::T3, t::T3),
            Keyframe::Off => (t::T1, t::T2),
            Keyframe::On => (t::AC, t::AC),
        };
        let live = !self.disabled;
        let arrow = |name: &'static str, icon: IconName, to: Option<Handler>| {
            div()
                .id(name)
                .flex()
                .items_center()
                .justify_center()
                .w(px(12.))
                .h(px(18.))
                .text_color(arrows)
                .child(Icon::new(icon).size(px(11.)))
                .when_some(to.filter(|_| live), |d, to| {
                    d.cursor_pointer()
                        .hover(|s| s.text_color(t::T1))
                        .on_click(move |_, window, cx| to(window, cx))
                })
        };
        h_flex()
            .id(self.id)
            .flex_none()
            .justify_end()
            .gap(px(1.))
            .when(self.disabled, |k| k.opacity(0.4))
            .child(arrow("prev", IconName::ChevronLeft, self.on_prev))
            .child(
                div()
                    .id("diamond")
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(16.))
                    .rounded(px(3.))
                    .child(diamond(self.keyframe, mark.into()))
                    .tooltip(Tooltip::text("Keyframe"))
                    .when_some(self.on_toggle.filter(|_| live), |d, toggle| {
                        d.cursor_pointer()
                            .hover(|s| s.bg(t::HV))
                            .on_click(move |_, window, cx| toggle(window, cx))
                    }),
            )
            .child(arrow("next", IconName::ChevronRight, self.on_next))
    }
}
