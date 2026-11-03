//! Floating surfaces: the raised card menus, select lists and popovers sit
//! on, a popover anchored to a button, and the titled panel ("Change speed")
//! a popover usually holds.

use super::{button::Button, icon::IconName, theme as t};
use gpui_kit::base::{Popover as BasePopover, PopoverState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::rc::Rc;

/// The raised card every floating layer is drawn on: `--r1` with a hairline
/// border, 8 px corners and the floating shadow.
pub fn surface() -> Div {
    div()
        .bg(t::R1)
        .border_1()
        .border_color(t::LN)
        .rounded(t::RADIUS_LG)
        .shadow(t::floating_shadow())
        .text_color(t::T1)
        .text_size(px(12.))
}

type OpenChange = Rc<dyn Fn(&bool, &mut Window, &mut App)>;
type Content =
    Box<dyn FnOnce(&mut PopoverState, &mut Window, &mut Context<PopoverState>) -> AnyElement>;

/// A popover anchored to a button: a click toggles it, Escape and a click
/// outside close it, and the button shows `selected` while it is open.
#[derive(IntoElement)]
pub struct Popover {
    id: ElementId,
    anchor: Anchor,
    open: Option<bool>,
    on_open_change: Option<OpenChange>,
    trigger: Option<Button>,
    content: Option<Content>,
    style: StyleRefinement,
}

impl Popover {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            anchor: Anchor::TopLeft,
            open: None,
            on_open_change: None,
            trigger: None,
            content: None,
            style: StyleRefinement::default(),
        }
    }

    /// The trigger corner the popover hangs from; below the left edge by
    /// default.
    pub fn anchor(mut self, anchor: Anchor) -> Self {
        self.anchor = anchor;
        self
    }

    /// Controls the open state; pair with [`Self::on_open_change`].
    pub fn open(mut self, open: bool) -> Self {
        self.open = Some(open);
        self
    }

    pub fn on_open_change(
        mut self,
        handler: impl Fn(&bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open_change = Some(Rc::new(handler));
        self
    }

    pub fn trigger(mut self, button: Button) -> Self {
        self.trigger = Some(button);
        self
    }

    /// What the surface holds, built each time it is drawn open.
    pub fn content<E: IntoElement>(
        mut self,
        content: impl FnOnce(&mut PopoverState, &mut Window, &mut Context<PopoverState>) -> E + 'static,
    ) -> Self {
        self.content = Some(Box::new(move |state, window, cx| {
            content(state, window, cx).into_any_element()
        }));
        self
    }
}

/// Styles the trigger's container, the box that takes part in the layout.
impl Styled for Popover {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Popover {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let trigger = self.trigger;
        let content = self.content;
        let mut popover = BasePopover::new(self.id)
            .anchor(self.anchor)
            .offset(px(6.))
            .trigger_with(move |open, _, _| match trigger {
                Some(button) if open => button.selected(true).into_any_element(),
                Some(button) => button.into_any_element(),
                None => div().into_any_element(),
            })
            .content(move |state, window, cx| {
                surface()
                    .occlude()
                    .when_some(content, |el, content| el.child(content(state, window, cx)))
            });
        if let Some(open) = self.open {
            popover = popover.open(open);
        }
        if let Some(handler) = self.on_open_change {
            popover = popover.on_open_change(move |open, window, cx| handler(open, window, cx));
        }
        popover.style().refine(&self.style);
        popover
    }
}

type CloseHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// A popover's titled layout: a 28 px header with the title and a close
/// button, the body, and an optional footer of buttons on the right.
#[derive(IntoElement)]
pub struct PopoverPanel {
    title: SharedString,
    on_close: Option<CloseHandler>,
    buttons: Vec<Button>,
    children: Vec<AnyElement>,
    style: StyleRefinement,
}

impl PopoverPanel {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            on_close: None,
            buttons: Vec::new(),
            children: Vec::new(),
            style: StyleRefinement::default(),
        }
    }

    /// Shows the header's close button.
    pub fn on_close(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_close = Some(Rc::new(handler));
        self
    }

    /// Adds a button to the footer, after the ones before it.
    pub fn button(mut self, button: Button) -> Self {
        self.buttons.push(button.small());
        self
    }
}

impl ParentElement for PopoverPanel {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

/// Styles the panel, usually its width.
impl Styled for PopoverPanel {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for PopoverPanel {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let mut panel = div().flex().flex_col();
        panel.style().refine(&self.style);
        panel
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h(t::PANEL_HEADER)
                    .pl(px(12.))
                    .pr(px(6.))
                    .border_b_1()
                    .border_color(t::LN)
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(div().flex_1().min_w_0().truncate().child(self.title))
                    .children(self.on_close.map(|handler| {
                        Button::new("popover-close")
                            .ghost()
                            .xsmall()
                            .icon(IconName::X)
                            .on_click(move |e, window, cx| handler(e, window, cx))
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .px(px(12.))
                    .py(px(10.))
                    .children(self.children),
            )
            .when(!self.buttons.is_empty(), |el| {
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_end()
                        .gap(px(6.))
                        .px(px(12.))
                        .py(px(8.))
                        .border_t_1()
                        .border_color(t::LN)
                        .children(self.buttons),
                )
            })
    }
}
