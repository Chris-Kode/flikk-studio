//! A sent message's text with its `@` references as chips: click one to
//! show its object in the editor, hover it for details.

use super::context;
use super::refs::{self, Mention, Segment};
use super::reveal::reveal;
use crate::state::EditorState;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
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
                let chip = chip((ix, n), m, state.clone(), cx).into_any_element();
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
        .gap_0p5()
        .children(lines.into_iter().map(|row| {
            h_flex()
                .flex_wrap()
                .gap_x(px(4.))
                .gap_y(px(2.))
                .min_h(rems(1.25))
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

/// A reference chip: kind icon and label, muted when it no longer resolves.
fn chip(id: (usize, usize), m: Mention, state: Entity<EditorState>, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let detail: SharedString = m.detail.clone().into();
    let token = m.token.clone();
    let accent = theme.accent;
    let color = if m.resolved {
        theme.foreground
    } else {
        theme.muted_foreground
    };
    h_flex()
        .id(ElementId::NamedInteger(
            "chat-ref".into(),
            (id.0 * 1024 + id.1) as u64,
        ))
        .flex_none()
        .max_w_full()
        .gap_1()
        .px_1()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
        .bg(theme.background)
        .text_color(color)
        .when(!m.resolved, |el| el.line_through())
        .when_some(m.kind, |el, kind| {
            el.child(
                Icon::new(kind.icon())
                    .size_3()
                    .flex_none()
                    .text_color(theme.muted_foreground),
            )
        })
        .child(div().min_w_0().truncate().child(m.label))
        .when(m.resolved, |el| {
            el.cursor_pointer()
                .hover(move |el| el.bg(accent))
                .on_click(move |_, _, cx| reveal(&state, &token, cx))
        })
        .when(!detail.is_empty(), |el| {
            el.tooltip(move |window, cx| Tooltip::new(detail.clone()).build(window, cx))
        })
}
