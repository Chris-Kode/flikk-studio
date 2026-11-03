//! Layout: resizable panel groups, whose dividers are the 1 px hairline
//! between panels (brighter under the pointer, signal blue while dragged),
//! and scroll areas with Flikk's thin scrollbar thumbs.

use super::theme as t;
use gpui_kit::base::{
    InteractiveElementExt as _, ResizablePanelGroup, ResizeHandleContext, ResizeHandleState,
    ScrollBounce, ScrollableMask, Scrollbar, ScrollbarAxis, ScrollbarHandle, ScrollbarStyles,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::panic::Location;
use std::rc::Rc;

pub use gpui_kit::base::resizable_panel;

/// Panels side by side, divided by draggable hairlines.
pub fn h_resizable(id: impl Into<ElementId>) -> ResizablePanelGroup {
    gpui_kit::base::h_resizable(id).with_handle_appearance(Rc::new(divider))
}

/// Panels stacked, divided by draggable hairlines.
pub fn v_resizable(id: impl Into<ElementId>) -> ResizablePanelGroup {
    gpui_kit::base::v_resizable(id).with_handle_appearance(Rc::new(divider))
}

fn divider(handle: &ResizeHandleContext, _: &mut Window, _: &mut App) -> Option<AnyElement> {
    let color = match handle.state() {
        ResizeHandleState::Idle => t::LN,
        ResizeHandleState::Hovered => t::LN_HV,
        ResizeHandleState::Pressed | ResizeHandleState::Dragging => t::AC,
    };
    let line = div().flex_none().bg(color);
    let line = if handle.axis() == Axis::Horizontal {
        line.h_full().w(px(1.))
    } else {
        line.w_full().h(px(1.))
    };
    Some(line.into_any_element())
}

/// Thin thumbs: 4 px at rest, 6 px under the pointer.
fn thumbs(styles: ScrollbarStyles) -> ScrollbarStyles {
    styles
        .track(|s| s.width(px(10.)))
        .thumb(|s| {
            s.bg(t::alpha(t::T1, 0.16))
                .width(px(4.))
                .inset(px(3.))
                .radius(px(2.))
                .min_length(px(32.))
        })
        .thumb_hover(|s| {
            s.bg(t::alpha(t::T1, 0.28))
                .width(px(6.))
                .inset(px(2.))
                .radius(px(3.))
        })
        .thumb_active(|s| {
            s.bg(t::alpha(t::T1, 0.36))
                .width(px(6.))
                .inset(px(2.))
                .radius(px(3.))
        })
}

fn scrollbar<H: ScrollbarHandle + Clone>(
    id: impl Into<ElementId>,
    handle: &H,
    axis: ScrollbarAxis,
) -> Div {
    div().absolute().inset_0().child(
        Scrollbar::new(handle)
            .id(id)
            .axis(axis)
            .viewport_from_layout()
            .styles(thumbs),
    )
}

#[track_caller]
fn caller_id() -> ElementId {
    ElementId::CodeLocation(*Location::caller())
}

/// Scrolling with Flikk scrollbars, for divs.
pub trait ScrollableElement: InteractiveElement + Styled + ParentElement + Element {
    /// Overlay a vertical scrollbar for content this element already scrolls,
    /// such as a `list`.
    #[track_caller]
    fn vertical_scrollbar<H: ScrollbarHandle + Clone>(self, handle: &H) -> Self {
        self.child(scrollbar(caller_id(), handle, ScrollbarAxis::Vertical))
    }

    /// Scroll the children vertically.
    #[track_caller]
    fn overflow_y_scrollbar(self) -> ScrollArea<Self> {
        ScrollArea::new(self, ScrollbarAxis::Vertical)
    }

    /// Scroll the children horizontally; a mouse wheel scrolls sideways.
    #[track_caller]
    fn overflow_x_scrollbar(self) -> ScrollArea<Self> {
        ScrollArea::new(self, ScrollbarAxis::Horizontal)
    }
}

impl ScrollableElement for Div {}

impl<E> ScrollableElement for Stateful<E>
where
    E: ParentElement + Styled + Element,
    Self: InteractiveElement,
{
}

/// An element whose children scroll inside it.
#[derive(IntoElement)]
pub struct ScrollArea<E: InteractiveElement + Styled + ParentElement + Element> {
    id: ElementId,
    element: E,
    axis: ScrollbarAxis,
}

impl<E: InteractiveElement + Styled + ParentElement + Element> ScrollArea<E> {
    #[track_caller]
    fn new(element: E, axis: ScrollbarAxis) -> Self {
        Self {
            id: caller_id(),
            element,
            axis,
        }
    }
}

impl<E: InteractiveElement + Styled + ParentElement + Element> Styled for ScrollArea<E> {
    fn style(&mut self) -> &mut StyleRefinement {
        self.element.style()
    }
}

impl<E: InteractiveElement + Styled + ParentElement + Element> ParentElement for ScrollArea<E> {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.element.extend(elements)
    }
}

impl<E: InteractiveElement + Styled + ParentElement + Element + 'static> RenderOnce
    for ScrollArea<E>
{
    fn render(mut self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let handle = window
            .use_keyed_state(self.id.clone(), cx, |_, _| ScrollHandle::default())
            .read(cx)
            .clone();
        let vertical = self.axis == ScrollbarAxis::Vertical;

        // The outer box takes the element's size and place in its parent;
        // the element itself grows with its content inside the scroller.
        let style = self.element.style();
        let mut outer = div().id(self.id.clone()).relative().size_full();
        outer.style().refine(&StyleRefinement {
            size: style.size.clone(),
            min_size: style.min_size.clone(),
            max_size: style.max_size.clone(),
            flex_grow: style.flex_grow,
            flex_shrink: style.flex_shrink,
            flex_basis: style.flex_basis,
            align_self: style.align_self,
            ..Default::default()
        });
        outer = outer.overflow_hidden();

        let content = self.element.flex_none();
        let content = if vertical {
            content.h_auto().min_h_full()
        } else {
            content.w_auto().min_w_full()
        };
        let area = div()
            .id((self.id.clone(), "area"))
            .size_full()
            .flex()
            .track_scroll(&handle);
        let area = if vertical {
            area.flex_col().overflow_y_scroll()
        } else {
            area.flex_row().overflow_x_scroll()
        }
        .lock_scroll_axis()
        .child(content);

        outer
            .child(ScrollBounce::new(
                (self.id.clone(), "bounce"),
                &handle,
                area,
            ))
            .when(!vertical, |outer| {
                outer.child(ScrollableMask::vertical_wheel(&handle))
            })
            .child(scrollbar((self.id, "scrollbar"), &handle, self.axis))
    }
}
