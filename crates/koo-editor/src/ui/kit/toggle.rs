//! Switch, checkbox and radio: a 26 × 14 track, a 14 px box and a 14 px
//! dot, each with an optional label to its right. The control and its label
//! are one click target; keyboard focus rings the control alone, and a
//! disabled control fades to 40 %.

use super::{icon::Icon, icon::IconName, theme as t, tooltip::Tooltip};
use gpui_kit::base::{
    Checkbox as BaseCheckbox, CheckboxState, Radio as BaseRadio, Switch as BaseSwitch, SwitchTrack,
};
use gpui_kit::*;
use std::rc::Rc;

type ChangeHandler = Rc<dyn Fn(&bool, &mut Window, &mut App)>;

/// Hover group shared by a control and its label.
const GROUP: &str = "flikk-toggle";

/// The control's focus handle, kept so the ring can hug the control rather
/// than the whole row.
fn focus_handle(id: &ElementId, window: &mut Window, cx: &mut App) -> FocusHandle {
    window
        .use_keyed_state(id.clone(), cx, |_, cx| cx.focus_handle())
        .read(cx)
        .clone()
}

fn focus_visible(focus: &FocusHandle, window: &Window) -> bool {
    focus.is_focused(window) && window.last_input_was_keyboard()
}

/// The row a control and its label share.
fn row<E: Styled + InteractiveElement>(el: E, disabled: bool) -> E {
    let el = el
        .group(GROUP)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(8.))
        .text_size(px(12.))
        .text_color(t::T1)
        .whitespace_nowrap();
    if disabled {
        el.opacity(0.4)
    } else {
        el.cursor_pointer()
    }
}

/// An on/off switch.
#[derive(IntoElement)]
pub struct Switch {
    id: ElementId,
    checked: bool,
    disabled: bool,
    label: Option<SharedString>,
    tooltip: Option<SharedString>,
    on_click: Option<ChangeHandler>,
}

impl Switch {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            checked: false,
            disabled: false,
            label: None,
            tooltip: None,
            on_click: None,
        }
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    /// Called with the requested value; the owner stores it and re-renders.
    pub fn on_click(mut self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Switch {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus = focus_handle(&self.id, window, cx);
        let ring = focus_visible(&focus, window);
        let (on, idle) = (self.checked, !self.checked && !self.disabled);

        let thumb = div()
            .absolute()
            .top(px(2.))
            .left(px(if on { 14. } else { 2. }))
            .size(px(10.))
            .rounded_full()
            .bg(if on { t::ON_AC } else { t::T2 });
        let thumb = if idle {
            thumb.group_hover(GROUP, |s| s.bg(t::T1))
        } else {
            thumb
        };
        let mut track = SwitchTrack::new((self.id.clone(), "track"))
            .checked(on)
            .disabled(self.disabled)
            .relative()
            .flex_none()
            .w(px(26.))
            .h(px(14.))
            .rounded_full()
            .bg(if on { t::AC } else { t::SW });
        if idle {
            track = track.group_hover(GROUP, |s| s.bg(t::SW_HV));
        }
        if ring {
            track = track.shadow(t::focus_ring());
        }

        let mut base = row(
            BaseSwitch::new(self.id).checked(on).disabled(self.disabled),
            self.disabled,
        )
        .track_focus(&focus);
        if let Some(label) = &self.label {
            base = base.accessibility_label(label.clone());
        }
        if let Some(handler) = self.on_click {
            base = base.on_change(move |next, _, window, cx| handler(&next, window, cx));
        }
        if let Some(text) = self.tooltip {
            base = base.tooltip(Tooltip::text(text));
        }
        base.child(track.child(thumb)).children(self.label)
    }
}

/// A checkbox: unchecked, checked, or indeterminate for a mixed group.
#[derive(IntoElement)]
pub struct Checkbox {
    id: ElementId,
    state: CheckboxState,
    disabled: bool,
    label: Option<SharedString>,
    on_click: Option<ChangeHandler>,
}

impl Checkbox {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            state: CheckboxState::Unchecked,
            disabled: false,
            label: None,
            on_click: None,
        }
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.state = if checked {
            CheckboxState::Checked
        } else {
            CheckboxState::Unchecked
        };
        self
    }

    /// Part of a group is checked: a dash. Clicking checks it.
    pub fn indeterminate(mut self, indeterminate: bool) -> Self {
        if indeterminate {
            self.state = CheckboxState::Indeterminate;
        }
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Called with the requested value; the owner stores it and re-renders.
    pub fn on_click(mut self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Checkbox {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus = focus_handle(&self.id, window, cx);
        let ring = focus_visible(&focus, window);
        let filled = self.state != CheckboxState::Unchecked;

        let mut mark = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(14.))
            .rounded(px(3.))
            .border_1();
        mark = if filled {
            mark.bg(t::AC).border_color(t::AC).text_color(t::ON_AC)
        } else {
            mark.bg(t::G).border_color(t::LN_CTL)
        };
        if !filled && !self.disabled {
            mark = mark.group_hover(GROUP, |s| s.bg(t::HV).border_color(t::LN_CTL_HV));
        }
        if ring {
            mark = mark.shadow(t::focus_ring());
        }
        mark = match self.state {
            CheckboxState::Checked => mark.child(Icon::new(IconName::Check).size(px(11.))),
            CheckboxState::Indeterminate => {
                mark.child(div().w(px(7.)).h(px(1.5)).rounded(px(1.)).bg(t::ON_AC))
            }
            CheckboxState::Unchecked => mark,
        };

        let mut base = row(
            BaseCheckbox::new(self.id)
                .state(self.state)
                .disabled(self.disabled),
            self.disabled,
        )
        .track_focus(&focus);
        if let Some(label) = &self.label {
            base = base.accessibility_label(label.clone());
        }
        if let Some(handler) = self.on_click {
            base = base.on_change(move |next, _, window, cx| {
                handler(&(next == CheckboxState::Checked), window, cx)
            });
        }
        base.child(mark).children(self.label)
    }
}

/// One choice of several. Grouping and the selected value are the owner's.
#[derive(IntoElement)]
pub struct Radio {
    id: ElementId,
    checked: bool,
    disabled: bool,
    label: Option<SharedString>,
    on_click: Option<ChangeHandler>,
}

impl Radio {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            checked: false,
            disabled: false,
            label: None,
            on_click: None,
        }
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Called with `true` when an unselected radio is picked.
    pub fn on_click(mut self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Radio {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus = focus_handle(&self.id, window, cx);
        let ring = focus_visible(&focus, window);

        let mut dot = div().flex_none().size(px(14.)).rounded_full();
        dot = if self.checked {
            dot.border(px(4.)).border_color(t::AC).bg(t::ON_AC)
        } else {
            dot.border_1().border_color(t::LN_CTL).bg(t::G)
        };
        if !self.checked && !self.disabled {
            dot = dot.group_hover(GROUP, |s| s.bg(t::HV).border_color(t::LN_CTL_HV));
        }
        if ring {
            dot = dot.shadow(t::focus_ring());
        }

        let mut base = row(
            BaseRadio::new(self.id)
                .checked(self.checked)
                .disabled(self.disabled),
            self.disabled,
        )
        .track_focus(&focus);
        if let Some(label) = &self.label {
            base = base.accessibility_label(label.clone());
        }
        if let Some(handler) = self.on_click {
            base = base.on_change(move |next, _, window, cx| handler(&next, window, cx));
        }
        base.child(dot).children(self.label)
    }
}
