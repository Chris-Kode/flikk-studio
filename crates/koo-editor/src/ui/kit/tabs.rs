//! Tabs. [`Tabs`] underlines the active page over a hairline (the
//! inspector's Video · Audio · Effects 2); [`PanelTabs`] names a panel's
//! pages in its header, dot-separated (Media · Effects · Voices).

use super::theme as t;
use gpui_kit::base::{Tab as BaseTab, Tabs as BaseTabs};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::rc::Rc;

type ClickHandler = Rc<dyn Fn(&usize, &mut Window, &mut App)>;

/// One page of [`Tabs`].
pub struct Tab {
    label: SharedString,
    count: usize,
    disabled: bool,
}

impl Tab {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            count: 0,
            disabled: false,
        }
    }

    /// How many items the page holds, after the label; hidden at zero.
    pub fn count(mut self, count: usize) -> Self {
        self.count = count;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// Underline tabs over a hairline.
#[derive(IntoElement)]
pub struct Tabs {
    id: ElementId,
    tabs: Vec<Tab>,
    selected: usize,
    on_click: Option<ClickHandler>,
    style: StyleRefinement,
}

impl Tabs {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            tabs: Vec::new(),
            selected: 0,
            on_click: None,
            style: StyleRefinement::default(),
        }
    }

    pub fn child(mut self, tab: Tab) -> Self {
        self.tabs.push(tab);
        self
    }

    pub fn children(mut self, tabs: impl IntoIterator<Item = Tab>) -> Self {
        self.tabs.extend(tabs);
        self
    }

    pub fn selected(mut self, ix: usize) -> Self {
        self.selected = ix;
        self
    }

    /// Called with the index of the tab clicked.
    pub fn on_click(mut self, handler: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl Styled for Tabs {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Tabs {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let count = self.tabs.len();
        let mut bar = BaseTabs::new(self.id)
            .flex()
            .flex_none()
            .gap(px(12.))
            .px(px(12.))
            .border_b_1()
            .border_color(t::LN)
            .text_size(px(11.));
        bar.style().refine(&self.style);

        bar.children(self.tabs.into_iter().enumerate().map(|(ix, tab)| {
            let selected = ix == self.selected;
            let mut item = BaseTab::new(ix)
                .selected(selected)
                .disabled(tab.disabled)
                .set_position(ix + 1, count)
                .relative()
                .h(px(29.))
                .gap(px(4.))
                .whitespace_nowrap()
                .text_color(if selected { t::T1 } else { t::T2 });
            if tab.disabled {
                item = item.opacity(0.4);
            } else if !selected {
                item = item.cursor_pointer().hover(|s| s.text_color(t::T1));
            }
            if let Some(handler) = self.on_click.clone() {
                item = item.on_click(move |_, window, cx| handler(&ix, window, cx));
            }
            item.child(tab.label)
                .when(tab.count > 0, |item| {
                    item.child(
                        div()
                            .font_family(t::mono())
                            .text_color(if selected { t::AC } else { t::T3 })
                            .child(tab.count.to_string()),
                    )
                })
                .when(selected, |item| {
                    item.child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .bottom_0()
                            .h(px(2.))
                            .bg(t::AC),
                    )
                })
        }))
    }
}

/// A panel's pages as dot-separated titles, for its header.
#[derive(IntoElement)]
pub struct PanelTabs {
    id: ElementId,
    items: Vec<SharedString>,
    selected: usize,
    on_click: Option<ClickHandler>,
}

impl PanelTabs {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            items: Vec::new(),
            selected: 0,
            on_click: None,
        }
    }

    pub fn item(mut self, label: impl Into<SharedString>) -> Self {
        self.items.push(label.into());
        self
    }

    pub fn items<L: Into<SharedString>>(mut self, labels: impl IntoIterator<Item = L>) -> Self {
        self.items.extend(labels.into_iter().map(Into::into));
        self
    }

    pub fn selected(mut self, ix: usize) -> Self {
        self.selected = ix;
        self
    }

    /// Called with the index of the page clicked.
    pub fn on_click(mut self, handler: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for PanelTabs {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let count = self.items.len();
        let mut bar = BaseTabs::new(self.id)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h(px(22.))
            .text_size(px(11.))
            .font_weight(FontWeight::MEDIUM)
            .whitespace_nowrap();
        for (ix, label) in self.items.into_iter().enumerate() {
            if ix > 0 {
                bar = bar.child(div().text_color(t::T3).child("·"));
            }
            let selected = ix == self.selected;
            let mut item = BaseTab::new(ix)
                .selected(selected)
                .set_position(ix + 1, count)
                .h_full()
                .text_color(if selected { t::T1 } else { t::T2 });
            if !selected {
                item = item.cursor_pointer().hover(|s| s.text_color(t::T1));
            }
            if let Some(handler) = self.on_click.clone() {
                item = item.on_click(move |_, window, cx| handler(&ix, window, cx));
            }
            bar = bar.child(item.child(label));
        }
        bar
    }
}
