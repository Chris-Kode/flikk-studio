//! Panel anatomy: the panel surface with its 28 px header (title, context,
//! trailing actions) and a 1 px accent line while it has focus; the
//! inspector's collapsible section header; and the 11 px caps label over a
//! group.

use super::{icon::Icon, icon::IconName, theme as t};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::rc::Rc;

/// A panel: the surface every workspace region sits on. Give it a
/// [`PanelHeader`] and its body as children.
#[derive(IntoElement)]
pub struct Panel {
    focused: bool,
    style: StyleRefinement,
    children: Vec<AnyElement>,
}

impl Panel {
    pub fn new() -> Self {
        Self {
            focused: false,
            style: StyleRefinement::default(),
            children: Vec::new(),
        }
    }

    /// Holds keyboard focus: the accent line along the top.
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }
}

impl Default for Panel {
    fn default() -> Self {
        Self::new()
    }
}

impl Styled for Panel {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl ParentElement for Panel {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Panel {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let mut panel = div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .min_w_0()
            .min_h_0()
            .bg(t::P)
            .text_color(t::T1)
            .text_size(px(12.));
        panel.style().refine(&self.style);
        panel.children(self.children).when(self.focused, |panel| {
            panel.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(px(1.))
                    .bg(t::AC),
            )
        })
    }
}

/// The 28 px header over a panel. Children are its trailing actions.
#[derive(IntoElement)]
pub struct PanelHeader {
    title: Option<AnyElement>,
    context: Option<SharedString>,
    actions: Vec<AnyElement>,
}

impl PanelHeader {
    pub fn new() -> Self {
        Self {
            title: None,
            context: None,
            actions: Vec::new(),
        }
    }

    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(
            div()
                .text_color(t::T1)
                .child(title.into())
                .into_any_element(),
        );
        self
    }

    /// In the title's place: the panel's page tabs or a compact segmented
    /// control.
    pub fn leading(mut self, element: impl IntoElement) -> Self {
        self.title = Some(element.into_any_element());
        self
    }

    /// What the panel shows, after the title: a clip, a timeline, a count.
    pub fn context(mut self, context: impl Into<SharedString>) -> Self {
        self.context = Some(context.into());
        self
    }
}

impl Default for PanelHeader {
    fn default() -> Self {
        Self::new()
    }
}

impl ParentElement for PanelHeader {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.actions.extend(elements);
    }
}

impl RenderOnce for PanelHeader {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h(t::PANEL_HEADER)
            .pl(px(12.))
            .pr(px(8.))
            .border_b_1()
            .border_color(t::LN)
            .text_size(px(11.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(t::T2)
            .whitespace_nowrap()
            .children(self.title)
            .children(
                self.context
                    .map(|c| div().min_w_0().truncate().text_color(t::T3).child(c)),
            )
            .child(div().flex_1())
            .children(self.actions)
    }
}

type ToggleHandler = Rc<dyn Fn(&bool, &mut Window, &mut App)>;

/// A section header in the inspector: chevron, title, trailing actions such
/// as reset or add. The chevron shows only when the section can collapse.
#[derive(IntoElement)]
pub struct SectionHeader {
    id: ElementId,
    title: SharedString,
    open: bool,
    on_toggle: Option<ToggleHandler>,
    actions: Vec<AnyElement>,
}

impl SectionHeader {
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            open: true,
            on_toggle: None,
            actions: Vec::new(),
        }
    }

    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Makes the section collapsible: called with the requested open state.
    pub fn on_toggle(mut self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Rc::new(handler));
        self
    }
}

impl ParentElement for SectionHeader {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.actions.extend(elements);
    }
}

impl RenderOnce for SectionHeader {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let open = self.open;
        let mut header = div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h(px(30.))
            .px(px(10.))
            .text_size(px(12.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(t::T1)
            .whitespace_nowrap();
        if let Some(handler) = self.on_toggle {
            header = header
                .cursor_pointer()
                .on_click(move |_, window, cx| handler(&!open, window, cx))
                .child(
                    Icon::new(if open {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .size(px(12.))
                    .color(t::T3),
                );
        }
        header
            .child(div().flex_1().min_w_0().truncate().child(self.title))
            .when(!self.actions.is_empty(), |header| {
                // The actions' clicks are theirs, not the header's toggle.
                header.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(2.))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .children(self.actions),
                )
            })
    }
}

/// The 11 px caps label over a group: RENDER QUEUE.
#[derive(IntoElement)]
pub struct SectionLabel {
    text: SharedString,
}

impl SectionLabel {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self { text: text.into() }
    }
}

impl RenderOnce for SectionLabel {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .flex_none()
            .text_size(px(11.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(t::T2)
            .whitespace_nowrap()
            .child(self.text.to_uppercase())
    }
}
