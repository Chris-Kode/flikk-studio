//! Effects library (Resolve-style): video effects, transitions, shader
//! generators, HTML titles and motion graphics, audio effects, the
//! project's own programs and the user's library of programs, presets and
//! sounds shared by every project, grouped by category and searchable. Items
//! drag onto the timeline and the chat; double-click applies to the
//! selection, or places a sound at the playhead.

mod apply;
mod glyphs;
pub mod shared;

pub use apply::{apply_fx, duplicate, fx_mention};
pub use glyphs::{FxGlyph, fx_glyph};

use crate::state::EditorState;
use crate::ui::media_pool::import::is_audio;
use crate::ui::{AudioFxDrag, FxDrag, SoundDrag};
use apply::Starter;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::LibraryId;
use koo_core::audiofx::AudioFxKind;
use koo_core::fx::{self, FxAsset, FxKind, FxLanguage, FxRef};
use koo_core::library::{Library, PRESET_EXT, PROGRAM_EXT, Preset, Sound};
use koo_core::model::Project;
use shared::Link;
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Section {
    Video,
    Transitions,
    Generators,
    Titles,
    Audio,
    Project,
    Library,
    Presets,
    Sounds,
}

impl Section {
    const ALL: [Section; 9] = [
        Self::Video,
        Self::Transitions,
        Self::Generators,
        Self::Titles,
        Self::Audio,
        Self::Project,
        Self::Library,
        Self::Presets,
        Self::Sounds,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Video => "Video Effects",
            Self::Transitions => "Transitions",
            Self::Generators => "Generators",
            Self::Titles => "Titles & Motion",
            Self::Audio => "Audio Effects",
            Self::Project => "Project",
            Self::Library => "Library",
            Self::Presets => "Presets",
            Self::Sounds => "Sounds",
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
    /// A program in the user's library.
    Program(LibraryId),
    Preset(LibraryId),
    Sound(LibraryId),
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

/// A project or library program's group: its category, else its kind.
fn program_category(a: &FxAsset) -> String {
    if a.category.trim().is_empty() {
        a.kind.label().to_owned() + "s"
    } else {
        a.category.clone()
    }
}

/// Presets group by what they hold.
fn preset_entry(p: &Preset) -> Entry {
    let (category, glyph) = if !p.effects.is_empty() {
        ("Effects", FxGlyph::Effect)
    } else if p.grade.is_some() {
        ("Color", FxGlyph::Effect)
    } else {
        ("Audio", FxGlyph::Audio)
    };
    Entry {
        item: Item::Preset(p.id),
        name: p.name.clone().into(),
        category: category.into(),
        description: SharedString::default(),
        glyph,
        kind: None,
    }
}

/// What a provider was asked for shows on hover.
fn sound_entry(s: &Sound) -> Entry {
    Entry {
        item: Item::Sound(s.id),
        name: s.name.clone().into(),
        category: s.category().into(),
        description: s
            .generated
            .as_ref()
            .map(|g| g.prompt.clone().into())
            .unwrap_or_default(),
        glyph: FxGlyph::Audio,
        kind: None,
    }
}

fn entries(project: &Project, lib: &Library, section: Section) -> Vec<Entry> {
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
                category: program_category(a),
                glyph: FxGlyph::Code,
                ..fx_entry(a)
            })
            .collect(),
        Section::Library => lib
            .programs
            .iter()
            .map(|p| Entry {
                item: Item::Program(p.id),
                category: program_category(&p.asset),
                glyph: FxGlyph::Code,
                ..fx_entry(&p.asset)
            })
            .collect(),
        Section::Presets => lib.presets.iter().map(preset_entry).collect(),
        Section::Sounds => lib.sounds.iter().map(sound_entry).collect(),
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
    /// Library item being renamed in `name`.
    renaming: Option<LibraryId>,
    name: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl LibraryPanel {
    pub fn new(state: Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        let name = cx.new(|cx| InputState::new(window, cx));
        let _subscriptions = vec![
            cx.observe(&state, |_, _, cx| cx.notify()),
            cx.subscribe(&search, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe_in(&name, window, |this, input, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    let name = input.read(cx).value().trim().to_owned();
                    this.finish_rename(name, cx);
                }
            }),
        ];
        Self {
            state,
            search,
            collapsed: HashSet::new(),
            selected: None,
            renaming: None,
            name,
            _subscriptions,
        }
    }

    fn apply(&mut self, item: &Item, cx: &mut Context<Self>) {
        let item = item.clone();
        self.state.update(cx, |s, cx| match &item {
            Item::Fx(r) => apply_fx(s, r, cx),
            Item::Audio(kind) => apply::apply_audio(s, *kind, cx),
            Item::Program(id) => shared::apply_program(s, *id, cx),
            Item::Preset(id) => shared::apply_preset(s, *id, cx),
            Item::Sound(id) => shared::place_sound(s, *id, cx),
        });
    }

    fn start_rename(&mut self, id: LibraryId, window: &mut Window, cx: &mut Context<Self>) {
        let lib = &self.state.read(cx).library;
        let Some(name) = lib
            .program(id)
            .map(|p| p.asset.name.clone())
            .or_else(|| lib.preset(id).map(|p| p.name.clone()))
            .or_else(|| lib.sound(id).map(|s| s.name.clone()))
        else {
            return;
        };
        self.renaming = Some(id);
        self.name.update(cx, |n, cx| {
            n.set_value(name, window, cx);
            n.focus(window, cx);
        });
        cx.notify();
    }

    fn finish_rename(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(id) = self.renaming.take() else {
            return;
        };
        self.state
            .update(cx, |s, cx| shared::rename(s, id, &name, cx));
        cx.notify();
    }

    /// Pick library files and copy them in.
    fn import(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: None,
        });
        let state = self.state.clone();
        cx.spawn(async move |_, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                let paths: Vec<_> = paths
                    .into_iter()
                    .filter(|p| {
                        p.extension()
                            .is_some_and(|e| e == PROGRAM_EXT || e == PRESET_EXT)
                            || is_audio(p)
                    })
                    .collect();
                state.update(cx, |s, cx| shared::import(s, &paths, cx));
            }
        })
        .detach();
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let state = self.state.clone();
        let panel = cx.entity();
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
                        let panel = panel.clone();
                        menu.item(PopupMenuItem::separator()).item(
                            PopupMenuItem::new("Import to Library…").on_click(move |_, _, cx| {
                                panel.update(cx, |this, cx| this.import(cx))
                            }),
                        )
                    }),
            )
    }

    fn section(&self, section: Section, query: &str, cx: &mut Context<Self>) -> Option<AnyElement> {
        let state = self.state.read(cx);
        let all = entries(state.bus.project(), &state.library, section);
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
            Item::Program(id) => format!("lib-{}", id.0),
            Item::Preset(id) => format!("preset-{}", id.0),
            Item::Sound(id) => format!("sound-{}", id.0),
        };
        let renaming = match &e.item {
            Item::Program(id) | Item::Preset(id) | Item::Sound(id) => self.renaming == Some(*id),
            _ => false,
        };
        if renaming {
            return h_flex()
                .h(px(26.))
                .pl(px(24.))
                .pr_2()
                .child(Input::new(&self.name).xsmall())
                .into_any_element();
        }
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
                    library: None,
                },
                |drag, _, _, cx| cx.new(|_| drag.clone()),
            ),
            (Item::Program(id), Some(kind)) => row.on_drag(
                FxDrag {
                    fx: FxRef::builtin(""),
                    kind,
                    name: e.name.clone(),
                    library: Some(*id),
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
            (Item::Sound(id), _) => row.on_drag(
                SoundDrag {
                    sound: *id,
                    name: e.name.clone(),
                },
                |drag, _, _, cx| cx.new(|_| drag.clone()),
            ),
            _ => row,
        };
        let (panel, state, item) = (cx.entity(), self.state.clone(), e.item.clone());
        row.context_menu(move |menu, _, cx| item_menu(menu, &panel, &state, &item, cx))
            .into_any_element()
    }
}

fn item_menu(
    menu: gpui_kit::component::menu::PopupMenu,
    panel: &Entity<LibraryPanel>,
    state: &Entity<EditorState>,
    item: &Item,
    cx: &App,
) -> gpui_kit::component::menu::PopupMenu {
    let (p, it) = (panel.clone(), item.clone());
    let sound = matches!(item, Item::Sound(_));
    let menu = menu.item(
        PopupMenuItem::new(if sound { "Add to Timeline" } else { "Apply" })
            .on_click(move |_, _, cx| p.update(cx, |this, cx| this.apply(&it, cx))),
    );
    let r = match item {
        Item::Fx(r) => r,
        Item::Audio(_) => return menu,
        Item::Program(id) => return library_menu(menu, panel, state, *id, true),
        Item::Preset(id) => return library_menu(menu, panel, state, *id, false),
        Item::Sound(id) => {
            let (id, s1, s2) = (*id, state.clone(), state.clone());
            let menu = menu
                .item(
                    PopupMenuItem::new("Add to Project").on_click(move |_, _, cx| {
                        s1.update(cx, |s, cx| {
                            shared::use_sound(s, id, cx);
                        })
                    }),
                )
                .item(
                    PopupMenuItem::new("Open in Viewer").on_click(move |_, _, cx| {
                        s2.update(cx, |s, cx| shared::open_sound(s, id, cx))
                    }),
                );
            return library_menu(menu, panel, state, id, false);
        }
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
    if builtin {
        return menu;
    }
    let link = shared::link(state.read(cx), r);
    let (s5, r5) = (state.clone(), r.clone());
    let save = PopupMenuItem::new(if link == Link::None {
        "Add to Library"
    } else {
        "Save to Library"
    })
    .on_click(move |_, _, cx| s5.update(cx, |s, cx| shared::save_program(s, &r5, cx)));
    menu = menu.item(PopupMenuItem::separator());
    if link != Link::Same {
        menu = menu.item(save);
    }
    if link == Link::Differs {
        let (s6, r6) = (state.clone(), r.clone());
        menu = menu.item(
            PopupMenuItem::new("Update from Library").on_click(move |_, _, cx| {
                s6.update(cx, |s, cx| shared::update_from_library(s, &r6, cx))
            }),
        );
    }
    let (s4, r4) = (state, r.clone());
    menu.item(PopupMenuItem::separator()).item(
        PopupMenuItem::new("Delete")
            .on_click(move |_, _, cx| s4.update(cx, |s, cx| apply::remove(s, &r4, cx))),
    )
}

/// What a library item offers besides using it.
fn library_menu(
    menu: gpui_kit::component::menu::PopupMenu,
    panel: &Entity<LibraryPanel>,
    state: &Entity<EditorState>,
    id: LibraryId,
    program: bool,
) -> gpui_kit::component::menu::PopupMenu {
    let mut menu = menu.item(PopupMenuItem::separator());
    if program {
        let (s1, s2) = (state.clone(), state.clone());
        menu = menu
            .item(
                PopupMenuItem::new("Edit code").on_click(move |_, _, cx| {
                    s1.update(cx, |s, cx| shared::edit_program(s, id, cx))
                }),
            )
            .item(PopupMenuItem::new("Duplicate").on_click(move |_, _, cx| {
                s2.update(cx, |s, cx| shared::duplicate_program(s, id, cx))
            }));
    }
    let (p, s3, s4) = (panel.clone(), state.clone(), state.clone());
    menu.item(
        PopupMenuItem::new("Rename").on_click(move |_, window, cx| {
            p.update(cx, |this, cx| this.start_rename(id, window, cx))
        }),
    )
    .item(
        PopupMenuItem::new("Show in Folder").on_click(move |_, _, cx| {
            let s = s3.read(cx);
            let audio = s.library.sound(id).map(Sound::audio);
            if let Some(path) = audio.or_else(|| shared::file_of(s, id)) {
                cx.reveal_path(&path);
            }
        }),
    )
    .item(PopupMenuItem::separator())
    .item(
        PopupMenuItem::new("Remove from Library")
            .on_click(move |_, _, cx| s4.update(cx, |s, cx| shared::remove(s, id, cx))),
    )
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
        v_flex().size_full().child(self.toolbar(cx)).child(
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
