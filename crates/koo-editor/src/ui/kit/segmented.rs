//! The segmented control: options on a sunken track, the chosen one raised.
//! Full size is the workspace switcher; compact sits in panel headers and
//! inspector rows (Source · Program, Clip · Timeline).

use super::theme as t;
use gpui_kit::base::{Tab as BaseTab, Tabs as BaseTabs};
use gpui_kit::*;
use std::rc::Rc;

type ClickHandler = Rc<dyn Fn(&usize, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct Segmented {
    id: ElementId,
    items: Vec<SharedString>,
    selected: usize,
    compact: bool,
    fill: bool,
    on_click: Option<ClickHandler>,
    style: StyleRefinement,
}

impl Segmented {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            items: Vec::new(),
            selected: 0,
            compact: false,
            fill: false,
            on_click: None,
            style: StyleRefinement::default(),
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

    /// 22 px, for panel headers and inspector rows.
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }

    /// Take the full width, splitting it evenly between the options.
    pub fn fill(mut self) -> Self {
        self.fill = true;
        self
    }

    /// Called with the index of the option clicked.
    pub fn on_click(mut self, handler: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl Styled for Segmented {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Segmented {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let compact = self.compact;
        let (h, pad, radius, item_radius, text) = if compact {
            (px(18.), px(8.), px(5.), px(4.), px(11.))
        } else {
            (px(24.), px(10.), t::RADIUS_SEG, t::RADIUS, px(12.))
        };
        let count = self.items.len();

        let mut track = BaseTabs::new(self.id)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(2.))
            .p(px(if compact { 1. } else { 2. }))
            .rounded(radius)
            .border_1()
            .border_color(t::LN)
            .bg(t::G)
            .text_size(text);
        if self.fill {
            track = track.w_full();
        }
        track.style().refine(&self.style);

        track.children(self.items.into_iter().enumerate().map(|(ix, label)| {
            let selected = ix == self.selected;
            let mut item = BaseTab::new(ix)
                .selected(selected)
                .set_position(ix + 1, count)
                .h(h)
                .px(pad)
                .rounded(item_radius)
                .whitespace_nowrap()
                .text_color(t::T2);
            if !compact {
                item = item.min_w(px(80.));
            }
            if self.fill {
                item = item.flex_1();
            }
            item = if selected {
                let item = item.bg(t::R1).text_color(t::T1);
                if compact {
                    item
                } else {
                    item.font_weight(FontWeight::MEDIUM).shadow(vec![BoxShadow {
                        color: t::alpha(t::ON_AC, 0.06).into(),
                        offset: point(px(0.), px(0.)),
                        blur_radius: px(0.),
                        spread_radius: px(1.),
                        inset: true,
                    }])
                }
            } else {
                item.cursor_pointer()
                    .hover(|s| s.bg(t::alpha(t::ON_AC, 0.03)).text_color(t::T1))
            };
            if let Some(handler) = self.on_click.clone() {
                item = item.on_click(move |_, window, cx| handler(&ix, window, cx));
            }
            item.child(label)
        }))
    }
}
