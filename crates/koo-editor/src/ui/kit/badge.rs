//! Pills and indicators. A [`Badge`] is an 18 px pill on a wash of its tone:
//! mono for change counts and diffs (+4, −2, 1 conflict), with a dot for a
//! status (Ready, Offline, Failed). An [`Indicator`] marks an icon button:
//! a dot for "something here", a count for how many.

use super::theme as t;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Tone {
    /// Grey on a raised surface: drafts, plain tags.
    #[default]
    Neutral,
    /// Added, ready, saved.
    Ok,
    /// Changed, offline, conflict.
    Warn,
    /// Removed, failed.
    Err,
}

impl Tone {
    fn solid(self) -> Rgba {
        match self {
            Tone::Neutral => t::T2,
            Tone::Ok => t::OK,
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
    dot: bool,
}

impl Badge {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            tone: Tone::default(),
            mono: false,
            dot: false,
        }
    }

    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    pub fn ok(self) -> Self {
        self.tone(Tone::Ok)
    }

    pub fn warn(self) -> Self {
        self.tone(Tone::Warn)
    }

    pub fn err(self) -> Self {
        self.tone(Tone::Err)
    }

    /// Numbers: change counts and diffs.
    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }

    /// A status: a dot before the label.
    pub fn dot(mut self) -> Self {
        self.dot = true;
        self
    }
}

impl RenderOnce for Badge {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let fg = self.tone.text();
        let bg = match self.tone {
            Tone::Neutral => t::R1,
            tone => t::alpha(tone.solid(), if self.dot { 0.1 } else { 0.12 }),
        };
        let pill = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.))
            .h(px(18.))
            .px(px(6.))
            .rounded(t::RADIUS_SM)
            .bg(bg)
            .text_color(fg)
            .text_size(px(11.))
            .whitespace_nowrap();
        let pill = if self.mono {
            pill.font_family(t::mono())
        } else {
            pill
        };
        pill.when(self.dot, |pill| {
            pill.child(div().flex_none().size(px(5.)).rounded_full().bg(fg))
        })
        .child(self.label)
    }
}

/// A mark in the corner of an icon button; add it as the button's child.
#[derive(IntoElement)]
pub struct Indicator {
    count: Option<usize>,
    tone: Tone,
}

impl Indicator {
    /// A 6 px dot in the tone's colour.
    pub fn dot(tone: Tone) -> Self {
        Self { count: None, tone }
    }

    /// A signal-blue count over the corner.
    pub fn count(count: usize) -> Self {
        Self {
            count: Some(count),
            tone: Tone::default(),
        }
    }
}

impl RenderOnce for Indicator {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        match self.count {
            None => div()
                .absolute()
                .top(px(4.))
                .right(px(4.))
                .size(px(6.))
                .rounded_full()
                .bg(self.tone.solid()),
            Some(count) => div()
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
