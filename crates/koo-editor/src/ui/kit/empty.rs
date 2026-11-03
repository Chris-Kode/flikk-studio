//! Empty states: an icon tile, a title, a hint and the actions that fill
//! the space, optionally inside a dashed drop zone. The compact form is a
//! plain icon over one line, for a panel that still shows content below.

use super::{icon::Icon, icon::IconName, theme as t};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

#[derive(IntoElement)]
pub struct EmptyState {
    icon: IconName,
    title: SharedString,
    hint: Option<SharedString>,
    drop_zone: bool,
    compact: bool,
    style: StyleRefinement,
    actions: Vec<AnyElement>,
}

impl EmptyState {
    pub fn new(icon: IconName, title: impl Into<SharedString>) -> Self {
        Self {
            icon,
            title: title.into(),
            hint: None,
            drop_zone: false,
            compact: false,
            style: StyleRefinement::default(),
            actions: Vec::new(),
        }
    }

    pub fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Files can be dropped here: a dashed outline inset from the panel.
    pub fn drop_zone(mut self) -> Self {
        self.drop_zone = true;
        self
    }

    /// A bare icon over the title, without tile, hint or actions.
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }
}

impl Styled for EmptyState {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl ParentElement for EmptyState {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.actions.extend(elements);
    }
}

impl RenderOnce for EmptyState {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let root = div().flex().flex_col().items_center().text_size(px(12.));
        let mut root = if self.compact {
            root.flex_none()
                .gap(px(6.))
                .pt(px(20.))
                .pb(px(16.))
                .text_color(t::T2)
                .child(Icon::new(self.icon).size(px(18.)).color(t::T3))
                .child(self.title)
        } else {
            let text = div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(3.))
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(t::T1)
                        .child(self.title),
                )
                .children(
                    self.hint
                        .map(|h| div().text_size(px(11.)).text_color(t::T3).child(h)),
                );
            root.flex_1()
                .justify_center()
                .gap(px(12.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(40.))
                        .rounded(px(10.))
                        .border_1()
                        .border_color(t::LN)
                        .bg(t::R1)
                        .child(Icon::new(self.icon).size(px(18.)).color(t::T2)),
                )
                .child(text)
                .when(!self.actions.is_empty(), |root| {
                    root.child(div().flex().gap(px(6.)).children(self.actions))
                })
                .when(self.drop_zone, |root| {
                    root.m(px(10.))
                        .rounded(px(6.))
                        .border_1()
                        .border_dashed()
                        .border_color(t::LN_DASHED)
                })
        };
        root.style().refine(&self.style);
        root
    }
}
