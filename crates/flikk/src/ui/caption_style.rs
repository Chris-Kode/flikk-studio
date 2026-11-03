//! Caption style editor (PRD C2), laid out like CapCut's text panel: the
//! profile's styles as chips over a preview, then ready-made looks, the
//! text itself (font, fill, stroke, background, shadow, placement, line
//! rules) and its animation (word highlight, intro, outro). Shown beside
//! the viewer in Captions and under a caption's text in the inspector.
//!
//! Edits save to the style profile (`EditorState::edit_caption_style`).
//! Scrubbed numbers and picked colours show in the viewer while they move
//! and save once they settle.

mod animation;
mod controls;
mod looks;
mod text;

use crate::state::EditorState;
use crate::ui::kit::theme as t;
use crate::ui::kit::{
    Button, ContextMenuExt as _, IconName, Input, NumberFieldState, PopupMenuItem, Segmented,
    SelectEvent, SelectState, h_flex, v_flex,
};
use controls::{Field, Paint};
use flikk_core::TrackId;
use flikk_core::memory::CaptionStyle;
use flikk_core::model::{ClipContent, TrackKind};
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::base::{ColorPickerEvent, ColorPickerState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::HashMap;
use std::time::Duration;

/// How long a color rests before it is saved.
const COLOR_SETTLE: Duration = Duration::from_millis(400);

type FontSelect = SelectState<SharedString>;

/// Which style the panel edits.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// The one new captions use (the first caption track's).
    Active,
    /// A caption track's.
    Track(TrackId),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Looks,
    Text,
    Animation,
}

const PAGES: [(Page, &str); 3] = [
    (Page::Looks, "Looks"),
    (Page::Text, "Text"),
    (Page::Animation, "Animation"),
];

pub struct CaptionStylePanel {
    state: Entity<EditorState>,
    target: Target,
    page: Page,
    numbers: HashMap<Field, Entity<NumberFieldState>>,
    /// Color pickers, with the color each last showed.
    colors: HashMap<Paint, (Entity<ColorPickerState>, flikk_core::memory::Color)>,
    color_pending: Option<PendingColor>,
    fonts: Entity<FontSelect>,
    /// Font the select was last set to.
    font_shown: String,
    /// Style being renamed in `name`.
    renaming: Option<usize>,
    name: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl CaptionStylePanel {
    pub fn new(
        state: Entity<EditorState>,
        target: Target,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let names: Vec<SharedString> = window
            .text_system()
            .all_font_names()
            .into_iter()
            .map(Into::into)
            .collect();
        let fonts = cx.new(|cx| SelectState::new(names, None, window, cx).searchable(true));
        let name = cx.new(|cx| InputState::new(window, cx));
        let mut _subscriptions = vec![
            cx.observe(&state, |_, _, cx| cx.notify()),
            cx.subscribe(&fonts, |this, _, ev: &SelectEvent<SharedString>, cx| {
                let SelectEvent::Confirm(font) = ev;
                let font = font.to_string();
                this.edit(move |s| s.font = font, cx);
            }),
            cx.subscribe_in(&name, window, |this, input, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    let name = input.read(cx).value().trim().to_owned();
                    this.finish_rename(name, cx);
                }
            }),
        ];
        let style = state.read(cx).active_caption_style();
        let mut colors = HashMap::new();
        for paint in Paint::ALL {
            let color = paint.get(&style);
            let picker =
                cx.new(|cx| ColorPickerState::new(window, cx).default_value(lettering_hsla(color)));
            _subscriptions.push(cx.subscribe(
                &picker,
                move |this, _, ev: &ColorPickerEvent, cx| {
                    if let ColorPickerEvent::Change(Some(c)) = ev {
                        this.pick_color(paint, controls::color_of(*c), cx);
                    }
                },
            ));
            colors.insert(paint, (picker, color));
        }
        let mut numbers = HashMap::new();
        for field in Field::ALL {
            let (number, sub) = controls::number(field, field.get(&style), window, cx);
            _subscriptions.push(sub);
            numbers.insert(field, number);
        }
        Self {
            state,
            target,
            page: Page::Text,
            numbers,
            colors,
            color_pending: None,
            fonts,
            font_shown: String::new(),
            renaming: None,
            name,
            _subscriptions,
        }
    }

    pub fn set_target(&mut self, target: Target, cx: &mut Context<Self>) {
        if self.target != target {
            self.target = target;
            self.renaming = None;
            cx.notify();
        }
    }

    /// Index of the edited style in the profile's presets.
    fn index(&self, cx: &App) -> usize {
        let state = self.state.read(cx);
        match self.target {
            Target::Active => state.caption_style,
            Target::Track(track) => state.track_caption_style(track),
        }
    }

    /// The edited style as shown, with any change still moving.
    fn style(&self, cx: &App) -> CaptionStyle {
        let i = self.index(cx);
        self.state
            .read(cx)
            .caption_styles()
            .get(i)
            .cloned()
            .unwrap_or_default()
    }

    /// Change the edited style and save it.
    fn edit(&mut self, f: impl FnOnce(&mut CaptionStyle), cx: &mut Context<Self>) {
        let i = self.index(cx);
        self.state
            .update(cx, |s, cx| s.edit_caption_style(i, f, cx));
    }

    /// Show a change in the viewer without saving it.
    fn show(&mut self, f: impl FnOnce(&mut CaptionStyle), cx: &mut Context<Self>) {
        let i = self.index(cx);
        let mut style = self.style(cx);
        f(&mut style);
        self.state
            .update(cx, |s, cx| s.show_caption_style(i, style, cx));
    }

    /// Show a picked color now; save it once the picker rests.
    fn pick_color(
        &mut self,
        paint: Paint,
        color: flikk_core::memory::Color,
        cx: &mut Context<Self>,
    ) {
        let style = self.index(cx);
        if let Some(pending) = self.color_pending.take()
            && (pending.style, pending.paint) != (style, paint)
        {
            pending.save(&self.state, cx);
        }
        self.show(|s| paint.set(s, color), cx);
        let timer = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COLOR_SETTLE).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(pending) = this.color_pending.take() {
                    pending.save(&this.state, cx);
                }
            });
        });
        self.color_pending = Some(PendingColor {
            style,
            paint,
            color,
            _timer: timer,
        });
    }

    /// Make preset `i` the edited style's: for new captions, or the track's.
    fn pick(&mut self, i: usize, cx: &mut Context<Self>) {
        let target = self.target;
        self.state.update(cx, |s, cx| match target {
            Target::Active => s.pick_caption_style(i, cx),
            Target::Track(track) => s.set_track_caption_style(track, i, cx),
        });
    }

    /// A copy of the edited style, picked.
    fn duplicate(&mut self, cx: &mut Context<Self>) {
        let mut style = self.style(cx);
        let names: Vec<String> = self
            .state
            .read(cx)
            .presets
            .caption_styles
            .iter()
            .map(|s| s.name.clone())
            .collect();
        style.name = (names.len() + 1..)
            .map(|n| format!("Style {n}"))
            .find(|n| !names.contains(n))
            .unwrap_or_default();
        self.state.update(cx, |s, cx| {
            s.edit_presets(|p| p.caption_styles.push(style), cx)
        });
        let i = self.state.read(cx).presets.caption_styles.len() - 1;
        self.pick(i, cx);
    }

    /// Remove preset `i`; tracks styled with it fall back to the first.
    fn delete(&mut self, i: usize, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            if s.presets.caption_styles.len() < 2 {
                return;
            }
            s.edit_presets(
                |p| {
                    p.caption_styles.remove(i);
                },
                cx,
            );
            if s.caption_style > i || s.caption_style >= s.presets.caption_styles.len() {
                s.caption_style = s.caption_style.saturating_sub(1);
            }
            let active = s.caption_style;
            s.pick_caption_style(active, cx);
        });
    }

    fn start_rename(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self
            .state
            .read(cx)
            .presets
            .caption_styles
            .get(i)
            .map(|s| s.name.clone())
        else {
            return;
        };
        self.renaming = Some(i);
        self.name.update(cx, |n, cx| {
            n.set_value(name, window, cx);
            n.focus(window, cx);
        });
        cx.notify();
    }

    /// Rename the style and the caption tracks that use it.
    fn finish_rename(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(i) = self.renaming.take() else {
            return;
        };
        cx.notify();
        let state = self.state.read(cx);
        let styles = &state.presets.caption_styles;
        let Some(old) = styles.get(i).map(|s| s.name.clone()) else {
            return;
        };
        if name.is_empty() || name == old || styles.iter().any(|s| s.name == name) {
            return;
        }
        let tracks: Vec<TrackId> = state
            .bus
            .project()
            .timeline
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Caption && t.caption_style.as_ref() == Some(&old))
            .map(|t| t.id)
            .collect();
        self.state.update(cx, |s, cx| {
            let renamed = name.clone();
            s.edit_caption_style(i, |style| style.name = renamed, cx);
            let commands = tracks
                .into_iter()
                .map(|track| flikk_core::command::Command::SetCaptionStyle {
                    track,
                    style: Some(name.clone()),
                })
                .collect();
            s.execute_batch(commands, "rename_caption_style", cx);
        });
    }

    /// Follow the model in the controls that keep their own state.
    fn sync(&mut self, style: &CaptionStyle, window: &mut Window, cx: &mut Context<Self>) {
        if self.font_shown != style.font {
            self.font_shown = style.font.clone();
            let font: SharedString = style.font.clone().into();
            self.fonts
                .update(cx, |f, cx| f.set_selected_value(&font, window, cx));
        }
        let pending = self.color_pending.as_ref().map(|p| p.paint);
        for (paint, (picker, shown)) in &mut self.colors {
            let color = paint.get(style);
            if *shown != color && pending != Some(*paint) {
                *shown = color;
                picker.update(cx, |p, cx| p.set_value(lettering_hsla(color), window, cx));
            }
        }
        for (field, number) in &self.numbers {
            let value = f64::from(field.get(style));
            number.update(cx, |n, cx| n.set_value(value, cx));
        }
    }

    /// The profile's styles; double-click renames, right-click for more.
    fn chips(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let names: Vec<String> = state
            .presets
            .caption_styles
            .iter()
            .map(|s| s.name.clone())
            .collect();
        let many = names.len() > 1;
        let active = self.index(cx);
        let panel = cx.entity();
        h_flex()
            .gap(px(4.))
            .flex_wrap()
            .children(names.into_iter().enumerate().map(|(i, name)| {
                if self.renaming == Some(i) {
                    return div()
                        .w(px(110.))
                        .child(Input::new(&self.name).small())
                        .into_any_element();
                }
                let panel = panel.clone();
                let on = i == active;
                div()
                    .id(("caption-style", i))
                    .flex()
                    .items_center()
                    .h(px(22.))
                    .px(px(8.))
                    .rounded(t::RADIUS)
                    .border_1()
                    .text_size(px(11.))
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .when(on, |d| {
                        d.bg(t::alpha(t::AC, 0.12))
                            .border_color(t::AC)
                            .text_color(t::T1)
                            .font_weight(FontWeight::MEDIUM)
                    })
                    .when(!on, |d| {
                        d.bg(t::R1)
                            .border_color(t::LN)
                            .text_color(t::T2)
                            .hover(|s| s.bg(t::HV).text_color(t::T1))
                    })
                    .child(name)
                    .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                        if ev.click_count() >= 2 {
                            this.start_rename(i, window, cx);
                        } else {
                            this.pick(i, cx);
                        }
                    }))
                    .context_menu(move |menu, _, _| {
                        let (rename, duplicate, delete) =
                            (panel.clone(), panel.clone(), panel.clone());
                        menu.item(PopupMenuItem::new("Rename").on_click(move |_, window, cx| {
                            rename.update(cx, |this, cx| this.start_rename(i, window, cx))
                        }))
                        .item(PopupMenuItem::new("Duplicate").on_click(move |_, _, cx| {
                            duplicate.update(cx, |this, cx| {
                                this.pick(i, cx);
                                this.duplicate(cx);
                            })
                        }))
                        .separator()
                        .item(
                            PopupMenuItem::new("Delete")
                                .destructive()
                                .disabled(!many)
                                .on_click(move |_, _, cx| {
                                    delete.update(cx, |this, cx| this.delete(i, cx))
                                }),
                        )
                    })
                    .into_any_element()
            }))
            .child(
                Button::new("add-caption-style")
                    .ghost()
                    .small()
                    .icon(IconName::Plus)
                    .tooltip("New style")
                    .on_click(cx.listener(|this, _, _, cx| this.duplicate(cx))),
            )
    }

    /// The caption under the playhead on the edited track, else its first,
    /// else sample text.
    fn sample(&self, cx: &App) -> String {
        let state = self.state.read(cx);
        let tl = &state.bus.project().timeline;
        let track = match self.target {
            Target::Track(id) => tl.track(id),
            Target::Active => tl.tracks.iter().find(|t| t.kind == TrackKind::Caption),
        };
        let text = |c: &flikk_core::model::Clip| match &c.content {
            ClipContent::Caption { text, .. } if !text.trim().is_empty() => Some(text.clone()),
            _ => None,
        };
        track
            .and_then(|t| {
                t.clip_at(state.playhead)
                    .and_then(text)
                    .or_else(|| t.clips.iter().find_map(text))
            })
            .unwrap_or_else(|| "Your captions look like this".into())
    }
}

/// A picked color shown but not yet saved, for preset `style`.
struct PendingColor {
    style: usize,
    paint: Paint,
    color: flikk_core::memory::Color,
    _timer: Task<()>,
}

impl PendingColor {
    fn save(self, state: &Entity<EditorState>, cx: &mut App) {
        let (paint, color) = (self.paint, self.color);
        state.update(cx, |s, cx| {
            s.edit_caption_style(self.style, |style| paint.set(style, color), cx)
        });
    }
}

fn lettering_hsla(c: flikk_core::memory::Color) -> Hsla {
    super::lettering::hsla(c, 1.)
}

impl Render for CaptionStylePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let style = self.style(cx);
        self.sync(&style, window, cx);
        let page = self.page;
        let body = match page {
            Page::Looks => self.looks_page(&style, cx).into_any_element(),
            Page::Text => self.text_page(&style, cx).into_any_element(),
            Page::Animation => self.animation_page(&style, cx).into_any_element(),
        };
        v_flex()
            .px(px(12.))
            .py(px(10.))
            .gap(px(10.))
            .child(self.chips(cx))
            .child(looks::preview(&style, self.sample(cx)))
            .child(
                Segmented::new("caption-style-page")
                    .compact()
                    .fill()
                    .items(PAGES.map(|(_, label)| label))
                    .selected(PAGES.iter().position(|(p, _)| *p == page).unwrap_or(0))
                    .on_click(cx.listener(|this, ix: &usize, _, cx| {
                        this.page = PAGES[(*ix).min(PAGES.len() - 1)].0;
                        cx.notify();
                    })),
            )
            .child(body)
    }
}
