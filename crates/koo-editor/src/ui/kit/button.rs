//! Buttons: primary, secondary, ghost and destructive, at 26, 22 or
//! 20 px, with an optional leading icon, label, shortcut chip and trailing
//! icon. Without a label a button is a square icon button.

use super::{icon::Icon, icon::IconName, kbd::Kbd, spinner::Spinner, theme as t, tooltip::Tooltip};
use gpui_kit::base::Button as BaseButton;
use gpui_kit::*;
use std::rc::Rc;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Variant {
    /// Raised: the default.
    #[default]
    Secondary,
    /// Signal blue: the one action a surface is for.
    Primary,
    /// Text and icon only until hovered.
    Ghost,
    /// Solid red: the confirm button of a destructive dialog.
    Destructive,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Size {
    /// 26 px.
    #[default]
    Md,
    /// 22 px.
    Sm,
    /// 20 px.
    Xs,
}

impl Size {
    fn height(self) -> Pixels {
        px(match self {
            Size::Md => 26.,
            Size::Sm => 22.,
            Size::Xs => 20.,
        })
    }
}

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    variant: Variant,
    size: Size,
    icon: Option<IconName>,
    label: Option<SharedString>,
    trailing: Option<IconName>,
    kbd: Option<SharedString>,
    tooltip: Option<(SharedString, Option<SharedString>)>,
    loading: bool,
    disabled: bool,
    selected: bool,
    accent: bool,
    focus: Option<FocusHandle>,
    on_click: Option<ClickHandler>,
    style: StyleRefinement,
    children: Vec<AnyElement>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            variant: Variant::default(),
            size: Size::default(),
            icon: None,
            label: None,
            trailing: None,
            kbd: None,
            tooltip: None,
            loading: false,
            disabled: false,
            selected: false,
            accent: false,
            focus: None,
            on_click: None,
            style: StyleRefinement::default(),
            children: Vec::new(),
        }
    }

    pub fn primary(mut self) -> Self {
        self.variant = Variant::Primary;
        self
    }

    pub fn ghost(mut self) -> Self {
        self.variant = Variant::Ghost;
        self
    }

    pub fn destructive(mut self) -> Self {
        self.variant = Variant::Destructive;
        self
    }

    /// 22 px.
    pub fn small(mut self) -> Self {
        self.size = Size::Sm;
        self
    }

    /// 20 px.
    pub fn xsmall(mut self) -> Self {
        self.size = Size::Xs;
        self
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// An icon after the label, such as a dropdown chevron.
    pub fn trailing_icon(mut self, icon: IconName) -> Self {
        self.trailing = Some(icon);
        self
    }

    /// The shortcut chip after the label.
    pub fn kbd(mut self, keys: impl Into<SharedString>) -> Self {
        self.kbd = Some(keys.into());
        self
    }

    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some((text.into(), None));
        self
    }

    /// A tooltip naming the button's shortcut.
    pub fn tooltip_kbd(
        mut self,
        text: impl Into<SharedString>,
        kbd: impl Into<SharedString>,
    ) -> Self {
        self.tooltip = Some((text.into(), Some(kbd.into())));
        self
    }

    /// Swaps the icon for a spinner and ignores clicks.
    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Toggled on, or its menu is open: the hover surface stays.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Toggled on: the icon turns signal blue (snap, linked selection).
    pub fn accent(mut self, accent: bool) -> Self {
        self.accent = accent;
        self
    }

    pub fn track_focus(mut self, focus: &FocusHandle) -> Self {
        self.focus = Some(focus.clone());
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl Styled for Button {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl ParentElement for Button {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Button {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let h = self.size.height();
        // An icon's children are marks over it, such as an `Indicator`.
        let square = self.label.is_none()
            && self.kbd.is_none()
            && (self.children.is_empty() || self.icon.is_some());
        let (pad, gap, text, icon) = match self.size {
            Size::Md => (px(10.), px(6.), px(12.), px(13.)),
            Size::Sm => (px(8.), px(5.), px(11.), px(13.)),
            Size::Xs => (px(6.), px(4.), px(11.), px(12.)),
        };
        let icon = if square {
            px(14.).min(h - px(8.))
        } else {
            icon
        };
        let on_fill = matches!(self.variant, Variant::Primary | Variant::Destructive);
        let inert = self.disabled || self.loading;

        let (bg, border, fg, hover, active): (Rgba, Rgba, Rgba, Rgba, Rgba) = match self.variant {
            Variant::Primary => (t::AC, t::AC, t::ON_AC, t::AC_HV, t::AC_PS),
            Variant::Secondary => (t::R1, t::LN, t::T1, t::HV, t::PS),
            Variant::Ghost => (t::alpha(t::G, 0.), t::alpha(t::G, 0.), t::T2, t::HV, t::PS),
            Variant::Destructive => (t::ERR, t::ERR, t::ON_AC, t::hex(0xFF8080), t::hex(0xE85A5A)),
        };
        let bg = match self.variant {
            _ if self.loading && on_fill => t::alpha(bg, 0.7),
            Variant::Ghost if self.selected => t::HV,
            Variant::Secondary if self.selected => t::HV,
            _ => bg,
        };
        let fg = match self.variant {
            Variant::Ghost if self.accent => t::AC,
            Variant::Ghost if self.selected => t::T1,
            _ => fg,
        };
        let hover_fg = match self.variant {
            Variant::Ghost if self.accent => t::AC,
            Variant::Ghost => t::T1,
            _ => fg,
        };

        let mut base = BaseButton::new(self.id)
            .disabled(inert)
            .selected(self.selected)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .gap(gap)
            .h(h)
            .rounded(if self.size == Size::Xs {
                t::RADIUS_SM
            } else {
                t::RADIUS
            })
            .border_1()
            .border_color(border)
            .bg(bg)
            .text_color(fg)
            .text_size(text)
            .font_weight(FontWeight::MEDIUM)
            .whitespace_nowrap()
            .focus_visible(|s| s.shadow(t::focus_ring()));
        base = if square { base.w(h) } else { base.px(pad) };
        if self.disabled {
            base = base.opacity(0.4);
        }
        if !inert {
            base = base
                .cursor_pointer()
                .hover(|s| s.bg(hover).text_color(hover_fg))
                .active(|s| s.bg(active));
        }
        if let Some(focus) = &self.focus {
            base = base.track_focus(focus);
        }
        if let Some(handler) = self.on_click {
            base = base.on_click(move |e, window, cx| handler(e, window, cx));
        }
        if let Some((text, kbd)) = self.tooltip {
            base = base.tooltip(Tooltip::with_kbd(text, kbd));
        }
        base.style().refine(&self.style);

        let base = match (self.loading, self.icon) {
            (true, _) => base.child(Spinner::new().size(icon)),
            (false, Some(name)) => base.child(Icon::new(name).size(icon)),
            (false, None) => base,
        };
        base.children(self.label)
            .children(self.children)
            .children(self.kbd.map(|k| Kbd::new(k).on_accent(on_fill)))
            .children(self.trailing.map(|name| Icon::new(name).size(px(11.))))
    }
}

impl Button {
    /// The id the menus and popovers it opens key their state by.
    pub(crate) fn element_id(&self) -> &ElementId {
        &self.id
    }
}
