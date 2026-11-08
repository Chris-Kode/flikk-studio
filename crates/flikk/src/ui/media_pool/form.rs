//! The rows of a form dialog (Video, Image, Voiceover, Generate, Voices): a
//! 96 px label column beside its control, a slider with its value, the line
//! that says why the form can't go ahead, and the tags a prompt paints in
//! the accent.

use std::ops::Range;

use crate::ui::kit::{Icon, IconName, Slider, h_flex, theme as t};
use gpui_kit::base::input::{InlineToken, InlineTokenContext, TextareaState};
use gpui_kit::base::slider::SliderState;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// Puts `tag` and a space into `input` at its cursor and focuses it. With
/// `set_off`, a space goes before it too when it would touch the word
/// before.
pub(crate) fn insert_tag(
    input: &Entity<TextareaState>,
    tag: &str,
    set_off: bool,
    window: &mut Window,
    cx: &mut App,
) {
    input.update(cx, |input, cx| {
        let value = input.value();
        let before = value.get(..input.cursor()).unwrap_or(&value);
        let word = before.chars().last().is_some_and(|c| !c.is_whitespace());
        let lead = if word && set_off { " " } else { "" };
        input.insert(format!("{lead}{tag} "), window, cx);
        input.focus(window, cx);
    });
}

/// `label` beside `control`.
pub(crate) fn row(label: impl Into<SharedString>, control: impl IntoElement) -> Div {
    h_flex()
        .gap(px(10.))
        .min_h(px(24.))
        .child(
            div()
                .w(px(96.))
                .flex_none()
                .text_color(t::T2)
                .child(label.into()),
        )
        .child(div().flex_1().min_w_0().child(control))
}

/// A slider with its value, mono, at the right.
pub(crate) fn slider(state: &Entity<SliderState>, value: impl Into<SharedString>) -> Div {
    h_flex()
        .gap(px(8.))
        .child(div().flex_1().min_w_0().child(Slider::new(state)))
        .child(
            h_flex()
                .w(px(40.))
                .flex_none()
                .justify_end()
                .font_family(t::mono())
                .text_size(px(11.))
                .text_color(t::T2)
                .child(value.into()),
        )
}

/// Why the form can't go ahead.
pub(crate) fn problem(text: impl Into<SharedString>) -> Div {
    h_flex()
        .items_start()
        .gap(px(8.))
        .text_color(t::ERR)
        .child(
            div()
                .flex_none()
                .pt(px(1.))
                .child(Icon::new(IconName::CircleAlert).size(px(14.))),
        )
        .child(div().flex_1().min_w_0().child(text.into()))
}

/// What a prompt marks as a tag.
#[derive(Clone, Copy)]
pub(crate) enum Tags {
    /// Audio tags in a script: `[whispers]`, `[short pause]`.
    Bracketed,
    /// References in a prompt: `@Image1`, `@Video2`.
    References,
}

impl Tags {
    /// Where the tags sit in `text`, in order.
    fn find(self, text: &str) -> Vec<Range<usize>> {
        match self {
            Tags::Bracketed => bracketed(text),
            Tags::References => references(text),
        }
    }
}

/// `[` words `]`, at most 40 bytes, with no space just inside the brackets.
fn bracketed(text: &str) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(open) = text[from..].find('[').map(|i| from + i) {
        let rest = &text[open + 1..];
        let Some(len) = rest.find(']') else { break };
        let inner = &rest[..len];
        let word = |c: char| c.is_alphanumeric() || matches!(c, ' ' | '-' | '\'');
        if len > 0
            && len <= 40
            && inner.chars().all(word)
            && !inner.starts_with(' ')
            && !inner.ends_with(' ')
        {
            found.push(open..open + len + 2);
            from = open + len + 2;
        } else {
            from = open + 1;
        }
    }
    found
}

/// `@` letters digits, at the start of the text or after a space.
fn references(text: &str) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    for (at, _) in text.match_indices('@') {
        if at > 0 && !bytes[at - 1].is_ascii_whitespace() {
            continue;
        }
        let letters = bytes[at + 1..]
            .iter()
            .take_while(|b| b.is_ascii_alphabetic())
            .count();
        let digits = bytes[at + 1 + letters..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        if letters > 0 && digits > 0 {
            found.push(at..at + 1 + letters + digits);
        }
    }
    found
}

/// Make each plain tag in `input` a token, so the field paints it in the
/// accent. The text and the caret stay as they were.
pub(crate) fn mark_tags(
    input: &Entity<TextareaState>,
    tags: Tags,
    window: &mut Window,
    cx: &mut App,
) {
    input.update(cx, |input, cx| {
        let text = input.value().to_string();
        let taken: Vec<Range<usize>> = input.tokens().iter().map(|s| s.range()).collect();
        let fresh: Vec<Range<usize>> = tags
            .find(&text)
            .into_iter()
            .filter(|r| !taken.iter().any(|t| t.start < r.end && r.start < t.end))
            .collect();
        if fresh.is_empty() {
            return;
        }
        let selection = input.selected_range();
        for range in fresh.into_iter().rev() {
            let tag = text[range.clone()].to_owned();
            let token = InlineToken::new(tag.clone(), tag);
            let _ = input.replace_range_with_token(range, token, window, cx);
        }
        input.set_selected_range(selection, cx);
    });
}

/// Take reference `tag` ("@Image2") out of `input` and number the later
/// ones of its kind down, as the references they name move up a place.
pub(crate) fn drop_tag(
    input: &Entity<TextareaState>,
    tag: &str,
    window: &mut Window,
    cx: &mut App,
) {
    let split = |tag: &str| {
        let tag = tag.strip_prefix('@')?;
        let at = tag.find(|c: char| c.is_ascii_digit())?;
        let n: usize = tag[at..].parse().ok()?;
        Some((tag[..at].to_owned(), n))
    };
    let Some((what, removed)) = split(tag) else {
        return;
    };
    input.update(cx, |input, cx| {
        let text = input.value().to_string();
        let mut cursor = input.cursor();
        let (mut out, mut copied) = (String::with_capacity(text.len()), 0);
        for range in references(&text) {
            let Some(n) = split(&text[range.clone()])
                .filter(|(w, n)| *w == what && *n >= removed)
                .map(|(_, n)| n)
            else {
                continue;
            };
            let (start, mut end) = (range.start, range.end);
            let new = if n == removed {
                // With the space after it.
                if text[end..].starts_with(' ') {
                    end += 1;
                }
                String::new()
            } else {
                format!("@{what}{}", n - 1)
            };
            if cursor >= end {
                cursor = cursor - (end - start) + new.len();
            } else if cursor > start {
                cursor = start + new.len();
            }
            out.push_str(&text[copied..start]);
            out.push_str(&new);
            copied = end;
        }
        if copied == 0 {
            return;
        }
        out.push_str(&text[copied..]);
        input.replace_all(out, window, cx);
        input.set_selected_range(cursor..cursor, cx);
    });
    mark_tags(input, Tags::References, window, cx);
}

/// A tag token: its text in the accent, washed while selected.
pub(crate) fn tag_token(token: &InlineTokenContext, _: &mut Window, _: &mut App) -> Div {
    div()
        .flex()
        .items_center()
        .h(token.line_height())
        .text_color(t::AC)
        .when(token.is_selected(), |d| d.bg(t::alpha(t::AC, 0.3)))
        .when(token.is_disabled(), |d| d.opacity(0.5))
        .child(token.token().label().clone())
}
