//! Text fields: the 26 px input (22 px small), its search variant, and the
//! multi-line textarea. Editing, focus and events stay in GPUI Base's
//! [`InputState`] and [`TextareaState`]; these elements only draw the frame
//! around them.

use super::{icon::Icon, icon::IconName, kbd::Kbd, theme as t};
use gpui_kit::base::input::{
    InlineTokenContext, InputBaseState, InputEditorStyle, InputModeKind, InputState, TextareaState,
};
use gpui_kit::base::{Input as BaseInput, InputBase, Textarea as BaseTextarea, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::rc::Rc;

/// Placeholder and selection colours for the editing engine, which paints
/// the text itself.
pub(super) fn project_style<M: InputModeKind>(state: &Entity<InputBaseState<M>>, cx: &mut App) {
    state.update(cx, |s, _| {
        s.set_editor_style(InputEditorStyle {
            muted_foreground: t::T3.into(),
            selection: t::alpha(t::AC, 0.3).into(),
            ..Default::default()
        })
    });
}

/// Border and wash of a field frame.
fn frame_colors(focused: bool) -> (Rgba, Option<Vec<BoxShadow>>) {
    if focused {
        (t::AC, Some(t::field_ring()))
    } else {
        (t::LN, None)
    }
}

/// Focus the field when its padding is clicked.
fn focus_on_click<M: InputModeKind>(
    state: &Entity<InputBaseState<M>>,
) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
    let state = state.clone();
    move |_, window, cx| {
        if !state.read(cx).has_selection_focus(window, cx) {
            state.update(cx, |s, cx| s.focus(window, cx));
        }
    }
}

/// A trailing icon that acts on the field, such as clear or reveal.
fn field_icon(id: &'static str, icon: IconName, color: Rgba) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(16.))
        .rounded(px(3.))
        .text_color(color)
        .cursor_pointer()
        .hover(|s| s.text_color(t::T1))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(Icon::new(icon).size(px(12.)))
}

/// A single-line text field.
#[derive(IntoElement)]
pub struct Input {
    state: Entity<InputState>,
    small: bool,
    search: bool,
    kbd: Option<SharedString>,
    mask_toggle: bool,
    disabled: bool,
    prefix: Option<AnyElement>,
    suffix: Option<AnyElement>,
    style: StyleRefinement,
}

impl Input {
    pub fn new(state: &Entity<InputState>) -> Self {
        Self {
            state: state.clone(),
            small: false,
            search: false,
            kbd: None,
            mask_toggle: false,
            disabled: false,
            prefix: None,
            suffix: None,
            style: StyleRefinement::default(),
        }
    }

    /// 22 px, 11 px text: inspector rows, popovers and toolbars.
    pub fn small(mut self) -> Self {
        self.small = true;
        self
    }

    /// A search icon in front and a clear button once something is typed.
    pub fn search(mut self) -> Self {
        self.search = true;
        self
    }

    /// The shortcut that focuses the field, shown while it is empty.
    pub fn kbd(mut self, keys: impl Into<SharedString>) -> Self {
        self.kbd = Some(keys.into());
        self
    }

    /// An eye button that reveals a masked value.
    pub fn mask_toggle(mut self) -> Self {
        self.mask_toggle = true;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Before the text, in the meta colour: an icon or a short label.
    pub fn prefix(mut self, prefix: impl IntoElement) -> Self {
        self.prefix = Some(prefix.into_any_element());
        self
    }

    /// After the text, in the meta colour: usually a unit such as `%`.
    pub fn suffix(mut self, suffix: impl IntoElement) -> Self {
        self.suffix = Some(suffix.into_any_element());
        self
    }
}

impl Styled for Input {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Input {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = self.state.clone();
        project_style(&state, cx);
        state.update(cx, |s, cx| s.set_disabled(self.disabled, cx));
        let s = state.read(cx);
        let focused = !self.disabled && s.has_selection_focus(window, cx);
        let empty = s.text().len() == 0;
        let masked = s.presentation().is_masked();
        let (h, pad, gap, text) = if self.small {
            (px(22.), px(6.), px(4.), px(11.))
        } else {
            (px(26.), px(8.), px(6.), px(12.))
        };
        let (border, ring) = frame_colors(focused);
        let idle = !focused && !self.disabled;

        let clear = (self.search && !empty && !self.disabled).then(|| {
            let state = state.clone();
            field_icon("clear", IconName::Close, t::T2).on_click(move |_, window, cx| {
                state.update(cx, |s, cx| {
                    s.clean(window, cx);
                    s.focus(window, cx);
                })
            })
        });
        let reveal = self.mask_toggle.then(|| {
            let state = state.clone();
            field_icon(
                "reveal",
                if masked {
                    IconName::Eye
                } else {
                    IconName::EyeOff
                },
                t::T3,
            )
            .on_click(move |_, window, cx| state.update(cx, |s, cx| s.toggle_masked(window, cx)))
        });

        let mut frame = InputBase::new(("flikk-input", state.entity_id()))
            .focused(focused)
            .disabled(self.disabled)
            .flex()
            .items_center()
            .gap(gap)
            .w_full()
            .h(h)
            .px(pad)
            .rounded(t::RADIUS_SM)
            .border_1()
            .border_color(border)
            .bg(t::G)
            .text_size(text)
            .line_height(px(16.))
            .text_color(t::T1)
            .when_some(ring, |f, ring| f.shadow(ring))
            .when(idle, |f| f.hover(|s| s.border_color(t::LN_HV)))
            .when(self.disabled, |f| f.opacity(0.4))
            .when(!self.disabled, |f| {
                f.cursor_text()
                    .on_mouse_down(MouseButton::Left, focus_on_click(&state))
            });
        frame.style().refine(&self.style);

        frame
            .when(self.search, |f| {
                f.child(Icon::new(IconName::Search).size(px(12.)).color(t::T3))
            })
            .children(
                self.prefix
                    .map(|p| h_flex().flex_none().text_color(t::T3).child(p)),
            )
            .child(div().flex_1().min_w_0().child(BaseInput::new(&state)))
            .when_some(self.kbd.filter(|_| empty && !focused), |f, keys| {
                f.child(div().text_color(t::T2).child(Kbd::new(keys)))
            })
            .children(clear)
            .children(reveal)
            .children(
                self.suffix
                    .map(|p| h_flex().flex_none().text_color(t::T3).child(p)),
            )
    }
}

type TokenRenderer = Rc<dyn Fn(&InlineTokenContext, &mut Window, &mut App) -> AnyElement>;

/// A multi-line text field, optionally with a `n / max` counter.
#[derive(IntoElement)]
pub struct Textarea {
    state: Entity<TextareaState>,
    small: bool,
    bare: bool,
    counter: Option<(usize, usize)>,
    token: Option<TokenRenderer>,
    style: StyleRefinement,
}

impl Textarea {
    pub fn new(state: &Entity<TextareaState>) -> Self {
        Self {
            state: state.clone(),
            small: false,
            bare: false,
            counter: None,
            token: None,
            style: StyleRefinement::default(),
        }
    }

    /// 11 px text and tighter padding, for inspector rows.
    pub fn small(mut self) -> Self {
        self.small = true;
        self
    }

    /// No frame: the text area sits inside a surface that draws its own.
    pub fn bare(mut self) -> Self {
        self.bare = true;
        self
    }

    /// `count / max` in the bottom corner, red once `count` passes `max`.
    pub fn counter(mut self, count: usize, max: usize) -> Self {
        self.counter = Some((count, max));
        self
    }

    /// The element each inline token renders as.
    pub fn token<R: IntoElement>(
        mut self,
        render: impl Fn(&InlineTokenContext, &mut Window, &mut App) -> R + 'static,
    ) -> Self {
        self.token = Some(Rc::new(move |token, window, cx| {
            render(token, window, cx).into_any_element()
        }));
        self
    }
}

impl Styled for Textarea {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Textarea {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = self.state.clone();
        let (py, pad, text, line) = match (self.bare, self.small) {
            (true, _) => (px(0.), px(0.), px(12.), px(18.)),
            (false, true) => (px(4.), px(6.), px(11.), px(16.)),
            (false, false) => (px(7.), px(9.), px(12.), px(18.)),
        };
        let counter_room = if self.counter.is_some() {
            px(14.)
        } else {
            px(0.)
        };
        project_style(&state, cx);
        state.update(cx, |s, _| {
            s.set_editor_paddings(Edges {
                top: py,
                right: pad,
                bottom: py + counter_room,
                left: pad,
            });
        });
        let focused = state.read(cx).has_selection_focus(window, cx);
        let (border, ring) = frame_colors(focused);
        let framed = !self.bare;

        let mut frame = InputBase::new(("flikk-textarea", state.entity_id()))
            .role(Role::MultilineTextInput)
            .focused(focused)
            .relative()
            .flex()
            .w_full()
            .h_auto()
            .text_size(text)
            .line_height(line)
            .text_color(t::T1)
            .when(framed, |f| {
                f.rounded(t::RADIUS_SM)
                    .border_1()
                    .border_color(border)
                    .bg(t::G)
                    .when_some(ring, |f, ring| f.shadow(ring))
                    .when(!focused, |f| f.hover(|s| s.border_color(t::LN_HV)))
            })
            .cursor_text()
            .on_mouse_down(MouseButton::Left, focus_on_click(&state));
        frame.style().refine(&self.style);

        let mut input = BaseTextarea::new(&state);
        if let Some(render) = self.token {
            input = input.token(move |token, window, cx| render(token, window, cx));
        }
        frame
            .child(div().relative().flex_1().min_w_0().child(input))
            .children(self.counter.map(|(count, max)| {
                div()
                    .absolute()
                    .right(px(8.))
                    .bottom(px(5.))
                    .font_family(t::mono())
                    .text_size(px(10.))
                    .line_height(px(12.))
                    .text_color(if count > max { t::ERR } else { t::T3 })
                    .child(format!("{count} / {max}"))
            }))
    }
}
