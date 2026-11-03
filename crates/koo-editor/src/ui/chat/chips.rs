//! Reference chips: a sent message's text with its `@` references as chips
//! (click one to show its object in the editor, hover it for details), and
//! the chip itself, which the composer's tokens share.

use super::context;
use super::refs::{self, Mark, Mention, Segment};
use super::reveal::reveal;
use crate::state::EditorState;
use crate::ui::kit::{Icon, Tooltip, h_flex, theme as t, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// Message `ix`'s text, line by line, words and chips wrapping together.
pub fn message_text(ix: usize, text: &str, state: &Entity<EditorState>, cx: &App) -> AnyElement {
    let s = state.read(cx);
    let project = s.bus.project();
    let analysis = s.services.analysis.as_deref();
    // Lines of glued groups: a word right after a chip (punctuation) or a
    // chip right after a word stays on the same line as it.
    let mut lines: Vec<Vec<Vec<AnyElement>>> = vec![Vec::new()];
    let mut glue = false;
    let mut n = 0;
    for segment in refs::segments(text, project) {
        match segment {
            Segment::Text(t) => {
                for (i, line) in t.split('\n').enumerate() {
                    if i > 0 {
                        lines.push(Vec::new());
                        glue = false;
                    }
                    let row = lines.last_mut().expect("one line at least");
                    for (j, word) in line.split_whitespace().enumerate() {
                        let word = div().child(word.to_owned()).into_any_element();
                        let attached = j == 0 && glue && !line.starts_with(char::is_whitespace);
                        match row.last_mut() {
                            Some(group) if attached => group.push(word),
                            _ => row.push(vec![word]),
                        }
                    }
                    glue = !line.is_empty() && !line.ends_with(char::is_whitespace);
                }
            }
            Segment::Ref(mut m) => {
                if m.kind == Some(refs::RefKind::Range)
                    && let Some(excerpt) = project_range(project, &m.token)
                        .and_then(|r| context::excerpt(project, analysis, r))
                {
                    m.detail = format!("{} · {excerpt}", m.detail);
                }
                n += 1;
                let mark = m.kind.map(|k| k.mark(project, &m.token));
                let chip = sent_chip((ix, n), m, mark, state.clone()).into_any_element();
                let row = lines.last_mut().expect("one line at least");
                match row.last_mut() {
                    Some(group) if glue => group.push(chip),
                    _ => row.push(vec![chip]),
                }
                glue = true;
            }
        }
    }
    v_flex()
        .gap(px(2.))
        .children(lines.into_iter().map(|row| {
            h_flex()
                .flex_wrap()
                .gap_x(px(4.))
                .gap_y(px(2.))
                .min_h(px(20.))
                .children(row.into_iter().map(|group| h_flex().children(group)))
        }))
        .into_any_element()
}

fn project_range(project: &koo_core::model::Project, token: &str) -> Option<koo_core::TimeRange> {
    match koo_core::query::resolve_project_references(project, token)
        .into_iter()
        .next()?
        .reference
    {
        koo_core::query::Reference::Range { range } => Some(range),
        _ => None,
    }
}

/// An 18 px reference chip: its mark and label.
pub fn chip(mark: Option<Mark>, label: impl Into<SharedString>) -> Div {
    h_flex()
        .flex_none()
        .max_w_full()
        .h(px(18.))
        .gap(px(4.))
        .px(px(5.))
        .rounded(t::RADIUS_SM)
        .border_1()
        .border_color(t::alpha(t::T1, 0.07))
        .bg(t::HV)
        .text_size(px(11.))
        .line_height(px(14.))
        .text_color(t::T1)
        .children(mark.map(|m| mark_element(m, px(7.), px(11.))))
        .child(div().min_w_0().truncate().child(label.into()))
}

/// A [`Mark`]: a `dot` px rounded square, or an `icon` px icon.
pub fn mark_element(mark: Mark, dot: Pixels, icon: Pixels) -> AnyElement {
    match mark {
        Mark::Dot(color) => div()
            .flex_none()
            .size(dot)
            .rounded(px(2.))
            .bg(color)
            .into_any_element(),
        Mark::Icon(name, color) => Icon::new(name).size(icon).color(color).into_any_element(),
    }
}

/// A chip in a sent message: crossed out when it no longer resolves.
fn sent_chip(
    id: (usize, usize),
    m: Mention,
    mark: Option<Mark>,
    state: Entity<EditorState>,
) -> impl IntoElement {
    let detail: SharedString = m.detail.clone().into();
    let token = m.token.clone();
    chip(mark, m.label)
        .id(ElementId::NamedInteger(
            "chat-ref".into(),
            (id.0 * 1024 + id.1) as u64,
        ))
        .when(!m.resolved, |el| el.text_color(t::T3).line_through())
        .when(m.resolved, |el| {
            el.cursor_pointer()
                .hover(|el| el.bg(t::PS))
                .on_click(move |_, _, cx| reveal(&state, &token, cx))
        })
        .when(!detail.is_empty(), |el| el.tooltip(Tooltip::text(detail)))
}
