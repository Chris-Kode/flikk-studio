//! Effects library (Resolve-style): video effects, transitions, shader
//! generators, HTML titles and motion graphics, audio effects and the
//! project's own programs, grouped by category and searchable. Items drag
//! onto the timeline and the chat; double-click applies to the selection.

mod apply;
mod glyphs;

pub use apply::{apply_fx, duplicate, fx_mention};
pub use glyphs::{FxGlyph, fx_glyph};

use crate::state::EditorState;
use crate::ui::{AudioFxDrag, FxDrag};
use apply::Starter;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::audiofx::AudioFxKind;
use koo_core::fx::{self, FxAsset, FxKind, FxLanguage, FxRef};
use koo_core::model::Project;
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Section {
    Video,
    Transitions,
    Generators,
    Titles,
    Audio,
    Project,
}

impl Section {
    const ALL: [Section; 6] = [
        Self::Video,
        Self::Transitions,
        Self::Generators,
        Self::Titles,
        Self::Audio,
        Self::Project,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Video => "Video Effects",
            Self::Transitions => "Transitions",
            Self::Generators => "Generators",
            Self::Titles => "Titles & Motion",
            Self::Audio => "Audio Effects",
            Self::Project => "Project",
        }
    }

    /// Where a built-in program is listed.
    fn of(asset: &FxAsset) -> Self {
        match (asset.kind, asset.language) {
            (FxKind::Effect, _) => Self::Video,
            (FxKind::Transition, _) => Self::Transitions,
            (FxKind::Generator, FxLanguage::Glsl) => Self::Generators,
            (FxKind::Generator, FxLanguage::Html) => Self::Titles,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Item {
    Fx(FxRef),
    Audio(AudioFxKind),
}

struct Entry {
    item: Item,
    name: SharedString,
    category: String,
    description: SharedString,
    glyph: FxGlyph,
    kind: Option<FxKind>,
}

pub fn kind_glyph(asset: &FxAsset) -> FxGlyph {
    match (asset.kind, asset.language) {
        (FxKind::Effect, _) => FxGlyph::Effect,
        (FxKind::Transition, _) => FxGlyph::Transition,
        (FxKind::Generator, FxLanguage::Glsl) => FxGlyph::Generator,
        (FxKind::Generator, FxLanguage::Html) => FxGlyph::Title,
    }
}

fn audio_category(kind: AudioFxKind) -> &'static str {
    use AudioFxKind::*;
    match kind {
        Equalizer | HighPass | LowPass => "EQ & Filters",
        Compressor | Limiter | Gate | DeEsser => "Dynamics",
        NoiseReduction => "Restoration",
        Reverb | Echo => "Space",
    }
}

fn audio_description(kind: AudioFxKind) -> &'static str {
    use AudioFxKind::*;
    match kind {
        Equalizer => "Low shelf, peaking mid and high shelf.",
        HighPass => "Removes rumble below the cutoff.",
        LowPass => "Softens highs above the cutoff.",
        Compressor => "Evens out loud and quiet passages.",
        Limiter => "Keeps peaks under the ceiling.",
        Gate => "Mutes the signal below the threshold.",
        DeEsser => "Tames harsh sibilance.",
        NoiseReduction => "Lowers steady background noise.",
        Reverb => "Adds room ambience.",
        Echo => "Repeats the sound after a delay.",
    }
}

fn entries(project: &Project, section: Section) -> Vec<Entry> {
    let fx_entry = |a: &FxAsset| Entry {
        item: Item::Fx(a.id.clone()),
        name: a.name.clone().into(),
        category: a.category.clone(),
        description: a.description.clone().into(),
        glyph: kind_glyph(a),
        kind: Some(a.kind),
    };
    match section {
        Section::Audio => AudioFxKind::ALL
            .into_iter()
            .map(|k| Entry {
                item: Item::Audio(k),
                name: k.label().into(),
                category: audio_category(k).into(),
                description: audio_description(k).into(),
                glyph: FxGlyph::Audio,
                kind: None,
            })
            .collect(),
        Section::Project => project
            .fx
            .iter()
            .map(|a| Entry {
                category: if a.category.trim().is_empty() {
                    a.kind.label().to_owned() + "s"
                } else {
                    a.category.clone()
                },
                glyph: FxGlyph::Code,
                ..fx_entry(a)
            })
            .collect(),
        _ => fx::builtins()
            .iter()
            .filter(|a| Section::of(a) == section)
            .map(fx_entry)
            .collect(),
    }
}

/// `entries` grouped by category, in first-seen order.
fn grouped(entries: Vec<Entry>) -> Vec<(String, Vec<Entry>)> {
    let mut groups: Vec<(String, Vec<Entry>)> = Vec::new();
    for e in entries {
        match groups.iter_mut().find(|(c, _)| *c == e.category) {
            Some((_, list)) => list.push(e),
            None => groups.push((e.category.clone(), vec![e])),
        }
    }
    groups
}

pub struct LibraryPanel {
    state: Entity<EditorState>,
    search: Entity<InputState>,
    collapsed: HashSet<Section>,
    selected: Option<Item>,
    _subscriptions: Vec<Subscription>,
}

impl LibraryPanel {
    pub fn new(state: Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        let _subscriptions = vec![
            cx.observe(&state, |_, _, cx| cx.notify()),
            cx.subscribe(&search, |_, _, _: &InputEvent, cx| cx.notify()),
        ];
        Self {
            state,
            search,
            collapsed: HashSet::new(),
            selected: None,
            _subscriptions,
        }
    }

    fn apply(&mut self, item: &Item, cx: &mut Context<Self>) {
        let item = item.clone();
        self.state.update(cx, |s, cx| match &item {
            Item::Fx(r) => apply_fx(s, r, cx),
            Item::Audio(kind) => apply::apply_audio(s, *kind, cx),
        });
    }

    fn toolbar(&self) -> impl IntoElement + use<> {
        let state = self.state.clone();
        h_flex()
            .gap_1()
            .p_2()
            .child(
                div().flex_1().child(
                    Input::new(&self.search)
                        .small()
                        .prefix(IconName::Search)
                        .cleanable(true),
                ),
            )
            .child(
                Button::new("new-program")
                    .ghost()
                    .small()
                    .icon(IconName::Plus)
                    .tooltip("New program")
                    .dropdown_menu(move |mut menu, _, _| {
                        for starter in Starter::ALL {
                            let state = state.clone();
                            menu = menu.item(PopupMenuItem::new(starter.label()).on_click(
                                move |_, _, cx| {
                                    state.update(cx, |s, cx| apply::new_program(s, starter, cx))
                                },
                            ));
                        }
                        menu
                    }),
            )
    }

    fn section(&self, section: Section, query: &str, cx: &mut Context<Self>) -> Option<AnyElement> {
        let project = self.state.read(cx).bus.project();
        let all = entries(project, section);
        let matches = |e: &Entry| {
            query.is_empty()
                || [e.name.as_ref(), e.category.as_str(), e.description.as_ref()]
                    .iter()
                    .any(|s| s.to_lowercase().contains(query))
        };
        let shown: Vec<Entry> = all.into_iter().filter(matches).collect();
        if shown.is_empty() {
            return None;
        }
        let count = shown.len();
        let open = !query.is_empty() || !self.collapsed.contains(&section);
        let theme = cx.theme();
        let (muted, hover) = (theme.muted_foreground, theme.list_hover);
        let header = h_flex()
            .id(SharedString::from(format!("section-{section:?}")))
            .h(px(26.))
            .px_1()
            .gap_1()
            .rounded_md()
            .cursor_pointer()
            .hover(|s| s.bg(hover))
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .child(
                Icon::new(if open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .xsmall()
                .text_color(muted),
            )
            .child(div().flex_1().child(section.label()))
            .child(div().text_color(muted).child(count.to_string()))
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.collapsed.remove(&section) {
                    this.collapsed.insert(section);
                }
                cx.notify();
            }));
        let mut body = v_flex().child(header);
        if open {
            for (category, list) in grouped(shown) {
                if !category.is_empty() {
                    body = body.child(
                        div()
                            .pl(px(26.))
                            .pt_1()
                            .pb_0p5()
                            .text_size(px(10.))
                            .text_color(muted)
                            .child(category),
                    );
                }
                for e in list {
                    body = body.child(self.row(e, cx));
                }
            }
        }
        Some(body.into_any_element())
    }

    fn row(&self, e: Entry, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let selected = self.selected.as_ref() == Some(&e.item);
        let (muted, fg) = (theme.muted_foreground, theme.foreground);
        let (active, hover) = (theme.list_active, theme.list_hover);
        let id = match &e.item {
            Item::Fx(r) => format!("fx-{r}"),
            Item::Audio(k) => format!("afx-{}", k.name()),
        };
        let item = e.item.clone();
        let description = e.description.clone();
        let row = h_flex()
            .id(SharedString::from(id))
            .h(px(26.))
            .pl(px(24.))
            .pr_2()
            .gap_2()
            .rounded_md()
            .text_xs()
            .cursor_pointer()
            .when(selected, |d| d.bg(active))
            .when(!selected, |d| d.hover(|s| s.bg(hover)))
            .child(fx_glyph(
                e.glyph,
                px(13.),
                if selected { fg } else { muted },
            ))
            .child(div().flex_1().min_w_0().truncate().child(e.name.clone()))
            .when(!description.is_empty(), |d| {
                d.tooltip(move |window, cx| Tooltip::new(description.clone()).build(window, cx))
            })
            .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                this.selected = Some(item.clone());
                if ev.click_count() >= 2 {
                    this.apply(&item, cx);
                }
                cx.notify();
            }));
        let row = match (&e.item, e.kind) {
            (Item::Fx(r), Some(kind)) => row.on_drag(
                FxDrag {
                    fx: r.clone(),
                    kind,
                    name: e.name.clone(),
                },
                |drag, _, _, cx| cx.new(|_| drag.clone()),
            ),
            (Item::Audio(kind), _) => row.on_drag(
                AudioFxDrag {
                    kind: *kind,
                    name: e.name.clone(),
                },
                |drag, _, _, cx| cx.new(|_| drag.clone()),
            ),
            _ => row,
        };
        let (panel, state, item) = (cx.entity(), self.state.clone(), e.item.clone());
        row.context_menu(move |menu, _, _| item_menu(menu, &panel, &state, &item))
            .into_any_element()
    }
}

fn item_menu(
    menu: gpui_kit::component::menu::PopupMenu,
    panel: &Entity<LibraryPanel>,
    state: &Entity<EditorState>,
    item: &Item,
) -> gpui_kit::component::menu::PopupMenu {
    let (p, it) = (panel.clone(), item.clone());
    let menu = menu.item(
        PopupMenuItem::new("Apply")
            .on_click(move |_, _, cx| p.update(cx, |this, cx| this.apply(&it, cx))),
    );
    let Item::Fx(r) = item else {
        return menu;
    };
    let state = state.clone();
    let (s1, r1) = (state.clone(), r.clone());
    let mut menu = menu
        .item(PopupMenuItem::new("Add to chat").on_click(move |_, _, cx| {
            s1.update(cx, |s, cx| {
                let mention = fx_mention(s.bus.project(), &r1);
                s.mention(vec![mention], cx)
            })
        }))
        .item(PopupMenuItem::separator());
    let builtin = matches!(r, FxRef::Builtin(_));
    let (s2, r2) = (state.clone(), r.clone());
    menu = menu.item(
        PopupMenuItem::new(if builtin { "View code" } else { "Edit code" })
            .on_click(move |_, _, cx| s2.update(cx, |s, cx| s.edit_fx(Some(r2.clone()), cx))),
    );
    let (s3, r3) = (state.clone(), r.clone());
    menu = menu.item(
        PopupMenuItem::new(if builtin {
            "Duplicate to edit"
        } else {
            "Duplicate"
        })
        .on_click(move |_, _, cx| s3.update(cx, |s, cx| duplicate(s, &r3, cx))),
    );
    if !builtin {
        let (s4, r4) = (state, r.clone());
        menu = menu.item(PopupMenuItem::separator()).item(
            PopupMenuItem::new("Delete")
                .on_click(move |_, _, cx| s4.update(cx, |s, cx| apply::remove(s, &r4, cx))),
        );
    }
    menu
}

impl Render for LibraryPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.search.read(cx).value().trim().to_lowercase();
        let sections: Vec<AnyElement> = Section::ALL
            .into_iter()
            .filter_map(|s| self.section(s, &query, cx))
            .collect();
        let muted = cx.theme().muted_foreground;
        let empty = sections.is_empty();
        v_flex().size_full().child(self.toolbar()).child(
            div()
                .id("library-items")
                .flex_1()
                .min_h_0()
                .px_1()
                .pb_2()
                .overflow_y_scrollbar()
                .when(empty, |d| {
                    d.flex()
                        .items_center()
                        .justify_center()
                        .text_color(muted)
                        .child(IconName::Search)
                })
                .children(sections),
        )
    }
}
