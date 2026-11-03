//! The select field: closed, a 26 px (22 px small) field with the picked
//! title and a chevron; open, a list on a floating surface with a check on
//! the picked row, optionally with a search field over it. Up/Down move,
//! Enter picks, Escape closes.

use super::{
    icon::{Icon, IconName},
    input::Input,
    popover::surface,
    theme as t,
};
use gpui_kit::base::actions::{Cancel, Confirm, SelectDown, SelectUp};
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::base::{ElementExt as _, Popup};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

const TRIGGER: &str = "FlikkSelect";
const LIST: &str = "FlikkSelectList";
/// Rows shown before the list scrolls.
const VISIBLE_ROWS: usize = 10;

pub(super) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", Confirm { secondary: false }, Some(TRIGGER)),
        KeyBinding::new("space", Confirm { secondary: false }, Some(TRIGGER)),
        KeyBinding::new("down", SelectDown, Some(TRIGGER)),
        KeyBinding::new("up", SelectUp, Some(LIST)),
        KeyBinding::new("down", SelectDown, Some(LIST)),
        KeyBinding::new("enter", Confirm { secondary: false }, Some(LIST)),
        KeyBinding::new("escape", Cancel, Some(LIST)),
        // The search field keeps typing; the list keeps these keys.
        KeyBinding::new("up", SelectUp, Some("FlikkSelectList > Input")),
        KeyBinding::new("down", SelectDown, Some("FlikkSelectList > Input")),
        KeyBinding::new(
            "enter",
            Confirm { secondary: false },
            Some("FlikkSelectList > Input"),
        ),
        KeyBinding::new("escape", Cancel, Some("FlikkSelectList > Input")),
    ]);
}

/// A row a select offers.
pub trait SelectItem: Clone + 'static {
    /// What a pick reports.
    type Value: Clone + PartialEq + 'static;

    fn title(&self) -> SharedString;

    fn value(&self) -> &Self::Value;
}

impl SelectItem for SharedString {
    type Value = SharedString;

    fn title(&self) -> SharedString {
        self.clone()
    }

    fn value(&self) -> &SharedString {
        self
    }
}

pub enum SelectEvent<T: SelectItem> {
    /// A row was picked, by the pointer or the keyboard.
    Confirm(T::Value),
}

/// The rows, the pick and the open list of a [`Select`].
pub struct SelectState<T: SelectItem> {
    focus_handle: FocusHandle,
    list_focus: FocusHandle,
    items: Vec<T>,
    selected: Option<usize>,
    searchable: bool,
    query: Entity<InputState>,
    /// Indices into `items` the search leaves.
    matches: Vec<usize>,
    /// Position in `matches`.
    highlighted: Option<usize>,
    open: bool,
    trigger_width: Pixels,
    scroll: UniformListScrollHandle,
    _query: Subscription,
}

impl<T: SelectItem> EventEmitter<SelectEvent<T>> for SelectState<T> {}

impl<T: SelectItem> SelectState<T> {
    pub fn new(
        items: Vec<T>,
        selected: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        let _query = cx.subscribe(&query, |this, _, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                this.refilter(cx);
            }
        });
        Self {
            focus_handle: cx.focus_handle(),
            list_focus: cx.focus_handle(),
            selected: selected.filter(|&ix| ix < items.len()),
            items,
            searchable: false,
            query,
            matches: Vec::new(),
            highlighted: None,
            open: false,
            trigger_width: px(0.),
            scroll: UniformListScrollHandle::new(),
            _query,
        }
    }

    /// A search field over the list.
    pub fn searchable(mut self, searchable: bool) -> Self {
        self.searchable = searchable;
        self
    }

    /// Replaces the rows; the pick is cleared.
    pub fn set_items(&mut self, items: Vec<T>, _: &mut Window, cx: &mut Context<Self>) {
        self.items = items;
        self.selected = None;
        self.refilter(cx);
    }

    pub fn set_selected_index(
        &mut self,
        ix: Option<usize>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected = ix.filter(|&ix| ix < self.items.len());
        cx.notify();
    }

    /// Picks the row reporting `value`, or none.
    pub fn set_selected_value(&mut self, value: &T::Value, _: &mut Window, cx: &mut Context<Self>) {
        self.selected = self.items.iter().position(|item| item.value() == value);
        cx.notify();
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.selected
    }

    pub fn selected_value(&self) -> Option<&T::Value> {
        Some(self.items.get(self.selected?)?.value())
    }

    fn refilter(&mut self, cx: &mut Context<Self>) {
        let query = self.query.read(cx).value().to_lowercase();
        self.matches = (0..self.items.len())
            .filter(|&ix| {
                query.is_empty() || self.items[ix].title().to_lowercase().contains(&query)
            })
            .collect();
        self.highlighted = self
            .selected
            .and_then(|s| self.matches.iter().position(|&ix| ix == s))
            .or_else(|| (!self.matches.is_empty()).then_some(0));
        if let Some(pos) = self.highlighted {
            self.scroll.scroll_to_item(pos, ScrollStrategy::Nearest);
        }
        cx.notify();
    }

    fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.close(window, cx);
            return;
        }
        self.open = true;
        if self.searchable {
            self.query.update(cx, |q, cx| q.set_value("", window, cx));
            self.query.read(cx).focus_handle(cx).focus(window, cx);
        } else {
            self.list_focus.focus(window, cx);
        }
        self.refilter(cx);
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.open = false;
            self.focus_handle.focus(window, cx);
            cx.notify();
        }
    }

    fn pick(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.items.get(ix) else {
            return;
        };
        let value = item.value().clone();
        self.selected = Some(ix);
        self.close(window, cx);
        cx.emit(SelectEvent::Confirm(value));
    }

    fn move_highlight(&mut self, down: bool, cx: &mut Context<Self>) {
        let n = self.matches.len();
        if n == 0 {
            return;
        }
        let pos = match (self.highlighted, down) {
            (None, true) => 0,
            (None, false) => n - 1,
            (Some(p), true) => (p + 1) % n,
            (Some(p), false) => (p + n - 1) % n,
        };
        self.highlighted = Some(pos);
        self.scroll.scroll_to_item(pos, ScrollStrategy::Nearest);
        cx.notify();
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Size {
    Md,
    Sm,
}

/// The field drawing a [`SelectState`]. It fills its parent's width.
#[derive(IntoElement)]
pub struct Select<T: SelectItem> {
    state: Entity<SelectState<T>>,
    size: Size,
    placeholder: Option<SharedString>,
    disabled: bool,
    style: StyleRefinement,
}

impl<T: SelectItem> Select<T> {
    pub fn new(state: &Entity<SelectState<T>>) -> Self {
        Self {
            state: state.clone(),
            size: Size::Md,
            placeholder: None,
            disabled: false,
            style: StyleRefinement::default(),
        }
    }

    /// 22 px.
    pub fn small(mut self) -> Self {
        self.size = Size::Sm;
        self
    }

    /// Shown while nothing is picked.
    pub fn placeholder(mut self, text: impl Into<SharedString>) -> Self {
        self.placeholder = Some(text.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl<T: SelectItem> Styled for Select<T> {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl<T: SelectItem> RenderOnce for Select<T> {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (h, pad, text, row_h) = match self.size {
            Size::Md => (px(26.), px(8.), px(12.), px(24.)),
            Size::Sm => (px(22.), px(6.), px(11.), px(22.)),
        };
        let state = self.state.clone();
        let s = self.state.read(cx);
        let disabled = self.disabled;
        let open = s.open && !disabled;
        let focus = s.focus_handle.clone();
        let lit = open || focus.is_focused(window);
        let title = s
            .selected
            .and_then(|ix| s.items.get(ix))
            .map(|item| item.title());
        let (label, label_fg) = match (title, self.placeholder) {
            (Some(title), _) => (title, t::T1),
            (None, placeholder) => (placeholder.unwrap_or_default(), t::T3),
        };

        let mut trigger = div()
            .id("select-trigger")
            .key_context(TRIGGER)
            .flex()
            .items_center()
            .gap(px(6.))
            .w_full()
            .h(h)
            .px(pad)
            .bg(t::G)
            .border_1()
            .border_color(if lit { t::AC } else { t::LN })
            .rounded(t::RADIUS_SM)
            .text_size(text)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(label_fg)
                    .child(label),
            )
            .child(
                Icon::new(if open {
                    IconName::ChevronUp
                } else {
                    IconName::ChevronDown
                })
                .size(px(14.).min(h - px(10.)))
                .color(if open { t::T2 } else { t::T3 }),
            )
            .on_prepaint({
                let state = state.clone();
                move |bounds, _, cx| state.update(cx, |s, _| s.trigger_width = bounds.size.width)
            });
        if lit {
            trigger = trigger.shadow(t::field_ring());
        }
        if disabled {
            trigger = trigger.opacity(0.4);
        } else {
            let (on_press, on_confirm, on_down) = (state.clone(), state.clone(), state.clone());
            trigger = trigger
                .track_focus(&focus.tab_stop(true))
                .cursor_pointer()
                .when(!open, |el| el.hover(|s| s.border_color(t::LN_HV)))
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    cx.stop_propagation();
                    // A press that already closed the list is not a reopen.
                    on_press.update(cx, |s, cx| {
                        if s.open == open {
                            s.toggle(window, cx)
                        }
                    })
                })
                .on_action(move |_: &Confirm, window, cx| {
                    on_confirm.update(cx, |s, cx| s.toggle(window, cx))
                })
                .on_action(move |_: &SelectDown, window, cx| {
                    on_down.update(cx, |s, cx| {
                        if !s.open {
                            s.toggle(window, cx)
                        }
                    })
                });
        }
        trigger.style().refine(&self.style);

        let popup = Popup::new(("select", self.state.entity_id().as_u64()), trigger)
            .anchor(Anchor::TopLeft)
            .offset(px(4.))
            .w_full();
        if !open {
            return popup;
        }

        let s = self.state.read(cx);
        let count = s.matches.len();
        let rows = {
            let state = state.clone();
            uniform_list("select-rows", count, move |range, _, cx| {
                let s = state.read(cx);
                range
                    .filter_map(|pos| {
                        let ix = *s.matches.get(pos)?;
                        let item = s.items.get(ix)?;
                        let (hover, pick) = (state.clone(), state.clone());
                        Some(
                            div()
                                .id(pos)
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .h(row_h)
                                .px(px(8.))
                                .rounded(t::RADIUS_SM)
                                .whitespace_nowrap()
                                .when(s.highlighted == Some(pos), |el| el.bg(t::HV))
                                .cursor_pointer()
                                .on_hover(move |hovered, _, cx| {
                                    if *hovered {
                                        hover.update(cx, |s, cx| {
                                            s.highlighted = Some(pos);
                                            cx.notify();
                                        })
                                    }
                                })
                                .on_click(move |_, window, cx| {
                                    pick.update(cx, |s, cx| s.pick(ix, window, cx))
                                })
                                .child(div().flex_1().min_w_0().truncate().child(item.title()))
                                .when(s.selected == Some(ix), |el| {
                                    el.child(Icon::new(IconName::Check).size(px(12.)).color(t::AC))
                                }),
                        )
                    })
                    .collect()
            })
            .h(row_h * count.min(VISIBLE_ROWS) as f32)
            .track_scroll(&s.scroll)
        };
        let (on_up, on_down, on_confirm, on_cancel, on_out) = (
            state.clone(),
            state.clone(),
            state.clone(),
            state.clone(),
            state.clone(),
        );
        let list = surface()
            .id("select-list")
            .key_context(LIST)
            .track_focus(&s.list_focus)
            .occlude()
            .flex()
            .flex_col()
            .gap(px(4.))
            .w(s.trigger_width)
            .min_w(px(120.))
            .p(px(4.))
            .text_size(text)
            .on_action(move |_: &SelectUp, _, cx| {
                on_up.update(cx, |s, cx| s.move_highlight(false, cx))
            })
            .on_action(move |_: &SelectDown, _, cx| {
                on_down.update(cx, |s, cx| s.move_highlight(true, cx))
            })
            .on_action(move |_: &Confirm, window, cx| {
                on_confirm.update(cx, |s, cx| {
                    if let Some(&ix) = s.highlighted.and_then(|pos| s.matches.get(pos)) {
                        s.pick(ix, window, cx)
                    }
                })
            })
            .on_action(move |_: &Cancel, window, cx| {
                on_cancel.update(cx, |s, cx| s.close(window, cx))
            })
            .on_mouse_down_out(move |_, window, cx| on_out.update(cx, |s, cx| s.close(window, cx)))
            .when(s.searchable, |el| {
                el.child(Input::new(&s.query).small().search())
            })
            .child(rows);
        popup.content(list)
    }
}
