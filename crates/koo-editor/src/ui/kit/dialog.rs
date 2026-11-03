//! Modal dialogs over a scrim. A confirm asks one question: a title, an
//! optional description, Cancel `Esc` and the confirm button `Enter`, which
//! can be destructive or working (spinner, nothing pressable). A form dialog has a title with a close button, the
//! caller's body, and the same footer when it has a confirm action.
//!
//! Open one with [`DialogExt::open_dialog`] or [`DialogExt::open_confirm`];
//! the builder runs on every frame, so the dialog follows the state it
//! reads. Escape cancels; Enter presses the focused footer button, else the
//! confirm button.

use super::{button::Button, icon::IconName, popover::surface, theme as t};
use gpui_kit::base::{Dialog as BaseDialog, Root, RootPlugin};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::rc::Rc;

pub(super) fn init(cx: &mut App) {
    Root::register_plugin::<DialogLayer>(cx, DialogLayer::new);
}

type Build = Rc<dyn Fn(Dialog, &mut Window, &mut App) -> Dialog>;
type Press = Rc<dyn Fn(&mut Window, &mut App) -> bool>;
type Close = Rc<dyn Fn(&mut Window, &mut App)>;

/// Opening and closing the window's dialogs.
pub trait DialogExt {
    /// A form dialog: title, close button, body, footer.
    fn open_dialog(
        &mut self,
        cx: &mut App,
        build: impl Fn(Dialog, &mut Window, &mut App) -> Dialog + 'static,
    );

    /// A confirm: title, description, Cancel and the confirm button, which
    /// opens focused.
    fn open_confirm(
        &mut self,
        cx: &mut App,
        build: impl Fn(Dialog, &mut Window, &mut App) -> Dialog + 'static,
    );

    /// Closes the topmost dialog.
    fn close_dialog(&mut self, cx: &mut App);

    fn has_active_dialog(&self, cx: &App) -> bool;
}

impl DialogExt for Window {
    fn open_dialog(
        &mut self,
        cx: &mut App,
        build: impl Fn(Dialog, &mut Window, &mut App) -> Dialog + 'static,
    ) {
        open(self, cx, false, Rc::new(build));
    }

    fn open_confirm(
        &mut self,
        cx: &mut App,
        build: impl Fn(Dialog, &mut Window, &mut App) -> Dialog + 'static,
    ) {
        open(self, cx, true, Rc::new(build));
    }

    fn close_dialog(&mut self, cx: &mut App) {
        if let Some(layer) = layer(self, cx) {
            layer.update(cx, |layer, cx| layer.close_top(self, cx));
        }
    }

    fn has_active_dialog(&self, cx: &App) -> bool {
        layer(self, cx).is_some_and(|layer| !layer.read(cx).dialogs.is_empty())
    }
}

fn layer(window: &Window, cx: &App) -> Option<Entity<DialogLayer>> {
    window.root::<Root>()??.read(cx).plugin::<DialogLayer>()
}

fn open(window: &mut Window, cx: &mut App, confirm: bool, build: Build) {
    let Some(layer) = layer(window, cx) else {
        return;
    };
    layer.update(cx, |layer, cx| {
        let dialog = Active {
            id: layer.next_id,
            confirm,
            focus: cx.focus_handle(),
            cancel: cx.focus_handle(),
            secondary: cx.focus_handle(),
            primary: cx.focus_handle(),
            previous: window.focused(cx).map(|f| f.downgrade()),
            build,
        };
        layer.next_id += 1;
        if confirm {
            dialog.primary.focus(window, cx);
        } else {
            dialog.focus.focus(window, cx);
        }
        layer.dialogs.push(dialog);
        cx.notify();
    });
}

/// What a dialog is built from, each frame.
pub struct Dialog {
    confirm: bool,
    title: Option<SharedString>,
    description: Option<SharedString>,
    close_button: bool,
    margin_top: Option<Pixels>,
    primary: Option<(SharedString, Press)>,
    secondary: Option<(SharedString, Press)>,
    destructive: bool,
    primary_disabled: bool,
    working: bool,
    children: Vec<AnyElement>,
    style: StyleRefinement,
}

impl Dialog {
    fn new(confirm: bool) -> Self {
        Self {
            confirm,
            title: None,
            description: None,
            close_button: !confirm,
            margin_top: None,
            primary: None,
            secondary: None,
            destructive: false,
            primary_disabled: false,
            working: false,
            children: Vec::new(),
            style: StyleRefinement::default(),
        }
    }

    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// A line under a confirm's title.
    pub fn description(mut self, text: impl Into<SharedString>) -> Self {
        self.description = Some(text.into());
        self
    }

    /// The header's close button; form dialogs have one by default.
    pub fn close_button(mut self, shown: bool) -> Self {
        self.close_button = shown;
        self
    }

    /// Hangs the dialog this far below the window top instead of centring
    /// it.
    pub fn margin_top(mut self, margin: Pixels) -> Self {
        self.margin_top = Some(margin);
        self
    }

    /// The confirm button, with Cancel before it. `press` runs on a click or
    /// Enter; the dialog closes when it returns `true`.
    pub fn confirm(
        mut self,
        label: impl Into<SharedString>,
        press: impl Fn(&mut Window, &mut App) -> bool + 'static,
    ) -> Self {
        self.primary = Some((label.into(), Rc::new(press)));
        self
    }

    /// A second choice between Cancel and confirm ("Don't Save").
    pub fn secondary(
        mut self,
        label: impl Into<SharedString>,
        press: impl Fn(&mut Window, &mut App) -> bool + 'static,
    ) -> Self {
        self.secondary = Some((label.into(), Rc::new(press)));
        self
    }

    /// A red confirm button.
    pub fn destructive(mut self) -> Self {
        self.destructive = true;
        self
    }

    /// Greys out the confirm button, such as until a form is filled.
    pub fn confirm_disabled(mut self, disabled: bool) -> Self {
        self.primary_disabled = disabled;
        self
    }

    /// The confirm button spins and nothing can be pressed, Escape included.
    pub fn working(mut self, working: bool) -> Self {
        self.working = working;
        self
    }
}

impl FluentBuilder for Dialog {}

impl ParentElement for Dialog {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

/// Styles the body, such as its width or padding.
impl Styled for Dialog {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

#[derive(Clone)]
struct Active {
    id: usize,
    confirm: bool,
    focus: FocusHandle,
    cancel: FocusHandle,
    secondary: FocusHandle,
    primary: FocusHandle,
    previous: Option<WeakFocusHandle>,
    build: Build,
}

/// The window's open dialogs, drawn over everything else.
struct DialogLayer {
    dialogs: Vec<Active>,
    next_id: usize,
}

impl RootPlugin for DialogLayer {}

impl DialogLayer {
    fn new(_: &mut Window, _: &mut Context<Self>) -> Self {
        Self {
            dialogs: Vec::new(),
            next_id: 0,
        }
    }

    fn close_top(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(top) = self.dialogs.last() {
            self.close(top.id, window, cx);
        }
    }

    /// Closes dialog `id` if it is still open, giving focus back to what had
    /// it before when it was on top.
    fn close(&mut self, id: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.dialogs.iter().position(|d| d.id == id) else {
            return;
        };
        let closed = self.dialogs.remove(ix);
        if ix == self.dialogs.len()
            && let Some(previous) = closed.previous.and_then(|f| f.upgrade())
        {
            previous.focus(window, cx);
        }
        cx.notify();
    }
}

impl Render for DialogLayer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let layer = cx.entity().downgrade();
        let dialogs = self.dialogs.clone();
        let top = dialogs.len().saturating_sub(1);
        div().children(dialogs.into_iter().enumerate().map(|(ix, active)| {
            let id = active.id;
            let close: Close = {
                let layer = layer.clone();
                Rc::new(move |window, cx| {
                    let _ = layer.update(cx, |layer, cx| layer.close(id, window, cx));
                })
            };
            let dialog = (active.build)(Dialog::new(active.confirm), window, cx);
            render_dialog(dialog, &active, ix, ix == top, close, window, cx)
        }))
    }
}

fn render_dialog(
    d: Dialog,
    active: &Active,
    layer: usize,
    top: bool,
    close: Close,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let working = d.working;
    let cancel: Close = {
        let close = close.clone();
        Rc::new(move |window, cx| {
            if !working {
                close(window, cx)
            }
        })
    };
    let press = |press: Option<Press>, enabled: bool| -> Option<Close> {
        let (press, close) = (press?, close.clone());
        Some(Rc::new(move |window: &mut Window, cx: &mut App| {
            if enabled && !working && press(window, cx) {
                close(window, cx)
            }
        }))
    };
    let primary = press(d.primary.as_ref().map(|p| p.1.clone()), !d.primary_disabled);
    let secondary = press(d.secondary.as_ref().map(|p| p.1.clone()), true);

    let (cancel_focus, secondary_focus) = (active.cancel.clone(), active.secondary.clone());
    let enter = {
        let (cancel, primary, secondary) = (cancel.clone(), primary.clone(), secondary.clone());
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            let pressed = if cancel_focus.is_focused(window) {
                Some(&cancel)
            } else if secondary_focus.is_focused(window) {
                secondary.as_ref()
            } else {
                primary.as_ref()
            };
            if let Some(pressed) = pressed {
                pressed(window, cx);
            }
            false
        }
    };
    let escape = {
        let cancel = cancel.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            cancel(window, cx);
            false
        }
    };

    let header = if d.confirm {
        Some(
            div()
                .flex()
                .flex_col()
                .px(px(18.))
                .pt(px(18.))
                .children(d.title.clone().map(title))
                .children(
                    d.description
                        .clone()
                        .map(|text| div().mt(px(6.)).text_color(t::T2).child(text)),
                ),
        )
    } else if d.title.is_some() || d.close_button {
        Some(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.))
                .px(px(16.))
                .pt(px(14.))
                .child(div().min_w_0().children(d.title.clone().map(title)))
                .when(d.close_button, |el| {
                    let cancel = cancel.clone();
                    el.child(
                        Button::new("dialog-close")
                            .ghost()
                            .xsmall()
                            .icon(IconName::X)
                            .disabled(working)
                            .on_click(move |_, window, cx| cancel(window, cx)),
                    )
                }),
        )
    } else {
        None
    };

    let footer = d.primary.as_ref().map(|(label, _)| {
        // The Enter chip follows whichever button Enter would press.
        let focus_on_other =
            active.cancel.is_focused(window) || active.secondary.is_focused(window);
        let on_cancel = cancel.clone();
        let primary_press = primary.clone();
        let mut confirm = Button::new("dialog-confirm")
            .label(label.clone())
            .track_focus(&active.primary)
            .loading(working)
            .disabled(d.primary_disabled)
            .on_click(move |_, window, cx| {
                if let Some(press) = &primary_press {
                    press(window, cx)
                }
            });
        confirm = if d.destructive {
            confirm.destructive()
        } else {
            confirm.primary()
        };
        if !focus_on_other && !working {
            confirm = confirm.kbd("Enter");
        }
        div()
            .flex()
            .items_center()
            .justify_end()
            .gap(px(8.))
            .mt(px(18.))
            .px(px(16.))
            .py(px(14.))
            .border_t_1()
            .border_color(t::LN)
            .child(
                Button::new("dialog-cancel")
                    .label("Cancel")
                    .kbd("Esc")
                    .track_focus(&active.cancel)
                    .disabled(working)
                    .on_click(move |_, window, cx| on_cancel(window, cx)),
            )
            .children(d.secondary.as_ref().map(|(label, _)| {
                let press = secondary.clone();
                Button::new("dialog-secondary")
                    .label(label.clone())
                    .track_focus(&active.secondary)
                    .disabled(working)
                    .on_click(move |_, window, cx| {
                        if let Some(press) = &press {
                            press(window, cx)
                        }
                    })
            }))
            .child(confirm)
    });
    let has_footer = footer.is_some();

    let body = (!d.confirm).then(|| {
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .px(px(16.))
            .pt(px(12.))
            .when(!has_footer, |el| el.pb(px(16.)))
            .children(d.children);
        body.style().refine(&d.style);
        body
    });

    let popup = surface()
        .id(("dialog", active.id))
        .occlude()
        .flex()
        .flex_col()
        .bg(t::P)
        .overflow_hidden()
        .when(d.confirm, |el| el.w(px(420.)))
        .when_some(d.margin_top, |el, margin| el.mt(margin))
        .children(header)
        .children(body)
        .children(footer);

    BaseDialog::new(cx)
        .focus_handle(active.focus.clone())
        .layer(layer, top)
        .close_on_backdrop_press(!working)
        .on_ok(enter)
        .on_cancel(escape)
        .backdrop(div().size_full().when(top, |el| el.bg(t::SCRIM)))
        .popup(popup)
        .when(d.margin_top.is_some(), |el| el.items_start())
        .into_any_element()
}

fn title(text: SharedString) -> Div {
    div()
        .text_size(px(13.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text)
}
