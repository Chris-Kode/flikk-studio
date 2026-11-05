//! Pills and indicators. A [`Badge`] is an 18 px pill on a wash of its tone,
//! mono for change counts and diffs (+4, −2). An [`Indicator`] marks an icon
//! button: a dot for "something here", a count for how many.

use super::theme as t;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Tone {
    /// Grey on a raised surface: drafts, plain tags.
    #[default]
    Neutral,
    /// Changed, offline, conflict.
    Warn,
    /// Removed, failed.
    Err,
}

impl Tone {
    fn solid(self) -> Rgba {
        match self {
            Tone::Neutral => t::T2,
            Tone::Warn => t::WARN,
            Tone::Err => t::ERR,
        }
    }

    fn text(self) -> Rgba {
        match self {
            Tone::Err => t::ERR_TX,
            tone => tone.solid(),
        }
    }
}

#[derive(IntoElement)]
pub struct Badge {
    label: SharedString,
    tone: Tone,
    mono: bool,
}

impl Badge {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            tone: Tone::default(),
            mono: false,
        }
    }

    pub fn warn(mut self) -> Self {
        self.tone = Tone::Warn;
        self
    }

    pub fn err(mut self) -> Self {
        self.tone = Tone::Err;
        self
    }

    /// Numbers: change counts and diffs.
    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }
}

impl RenderOnce for Badge {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let fg = self.tone.text();
        let bg = match self.tone {
            Tone::Neutral => t::R1,
            tone => t::alpha(tone.solid(), 0.12),
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .h(px(18.))
            .px(px(6.))
            .rounded(t::RADIUS_SM)
            .bg(bg)
            .text_color(fg)
            .text_size(px(11.))
            .whitespace_nowrap()
            .when(self.mono, |pill| pill.font_family(t::mono()))
            .child(self.label)
    }
}

/// A mark in the corner of an icon button; add it as the button's child.
#[derive(IntoElement)]
pub enum Indicator {
    /// A 6 px dot in the tone's colour.
    Dot(Tone),
    /// A signal-blue count over the corner.
    Count(usize),
}

impl RenderOnce for Indicator {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        match self {
            Indicator::Dot(tone) => div()
                .absolute()
                .top(px(4.))
                .right(px(4.))
                .size(px(6.))
                .rounded_full()
                .bg(tone.solid()),
            Indicator::Count(count) => div()
                .absolute()
                .top(px(-4.))
                .right(px(-4.))
                .flex()
                .items_center()
                .justify_center()
                .min_w(px(14.))
                .h(px(14.))
                .px(px(2.))
                .rounded_full()
                .border_2()
                .border_color(t::P)
                .bg(t::AC)
                .text_color(t::ON_AC)
                .text_size(px(9.))
                .font_weight(FontWeight::SEMIBOLD)
                .line_height(px(10.))
                .child(count.to_string()),
        }
    }
}
