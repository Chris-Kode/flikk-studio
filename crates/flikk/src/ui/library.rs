//! Effects library (Resolve-style): video effects, transitions, shader
//! generators, HTML titles and motion graphics, audio effects, the
//! project's own programs and the user's library of programs, presets,
//! sounds and voices shared by every project, grouped by category and
//! searchable. Items drag onto the timeline and the chat; double-click
//! applies to the selection, places a sound at the playhead, or opens the
//! voiceover form on a voice. A voice gone from the account shows dimmed.

mod apply;
mod glyphs;
pub mod shared;

pub use apply::{apply_fx, duplicate, fx_mention};
pub use glyphs::{FxGlyph, fx_glyph};

use crate::state::EditorState;
use crate::ui::kit::theme as t;
use crate::ui::kit::{
    Button, ContextMenuExt as _, DialogExt as _, DropdownMenu as _, EmptyState, Icon, IconName,
    Input, PopupMenu, PopupMenuItem, ScrollableElement as _, Tab, Tabs, Tooltip, h_flex, v_flex,
};
use crate::ui::media_pool::import::is_audio;
use crate::ui::media_pool::{clock, listen_dot};
use crate::ui::{AudioFxDrag, FxDrag, SoundDrag};
use apply::Starter;
use flikk_core::LibraryId;
use flikk_core::audiofx::AudioFxKind;
use flikk_core::cloud::VoiceOrigin;
use flikk_core::fx::{self, FxAsset, FxKind, FxLanguage, FxRef};
use flikk_core::library::{Library, PRESET_EXT, PROGRAM_EXT, Preset, SavedVoice, Sound};
use flikk_core::model::Project;
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use shared::Link;
use std::cell::Cell;
use std::rc::Rc;

/// Narrowest an effect tile gets before the grid drops a column.
const TILE: f32 = 72.;

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
    Voices,
}

impl Section {
    const ALL: [Section; 10] = [
        Self::Video,
        Self::Transitions,
        Self::Generators,
        Self::Titles,
        Self::Audio,
        Self::Project,
        Self::Library,
        Self::Presets,
        Self::Sounds,
        Self::Voices,
    ];

    /// Its page's name in the tabs.
    fn tab(self) -> &'static str {
        match self {
            Self::Video => "Video",
            Self::Transitions => "Transitions",
            Self::Generators => "Generators",
            Self::Titles => "Titles",
            Self::Audio => "Audio",
            Self::Project => "Project",
            Self::Library => "Library",
            Self::Presets => "Presets",
            Self::Sounds => "Sounds",
            Self::Voices => "Voices",
        }
    }

    /// Its heading over the items.
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
            Self::Voices => "Voices",
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
    Voice(LibraryId),
}

struct Entry {
    item: Item,
    name: SharedString,
    category: String,
    description: SharedString,
    glyph: FxGlyph,
    kind: Option<FxKind>,
    /// What a row shows after the name: a sound's length, a voice's kind.
    meta: Option<SharedString>,
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
        meta: None,
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
        meta: s.info.as_ref().map(|i| clock(i.duration).into()),
    }
}

/// How it was made shows on hover.
fn voice_entry(v: &SavedVoice) -> Entry {
    Entry {
        item: Item::Voice(v.id),
        name: v.name.clone().into(),
        category: v.category().into(),
        description: v.description().into(),
        glyph: FxGlyph::Audio,
        kind: None,
        meta: None,
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
        meta: None,
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
                meta: None,
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
        Section::Voices => lib.voices.iter().map(voice_entry).collect(),
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
    /// The page shown; `None` shows every section.
    page: Option<Section>,
    selected: Option<Item>,
    /// Library item being renamed in `name`.
    renaming: Option<LibraryId>,
    name: Entity<InputState>,
    /// The items area's width last frame, for the tiles' columns.
    width: Rc<Cell<Pixels>>,
    _subscriptions: Vec<Subscription>,
}

impl LibraryPanel {
    pub fn new(state: Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search effects, sounds, voices"));
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
        if !state.read(cx).library.voices.is_empty() {
            state.update(cx, |s, cx| s.want_voices(cx));
        }
        Self {
            state,
            search,
            page: None,
            selected: None,
            renaming: None,
            name,
            width: Rc::default(),
            _subscriptions,
        }
    }

    fn apply(&mut self, item: &Item, window: &mut Window, cx: &mut Context<Self>) {
        let item = item.clone();
        if let Item::Voice(id) = item {
            return read_with(&self.state, id, window, cx);
        }
        self.state.update(cx, |s, cx| match &item {
            Item::Fx(r) => apply_fx(s, r, cx),
            Item::Audio(kind) => apply::apply_audio(s, *kind, cx),
            Item::Program(id) => shared::apply_program(s, *id, cx),
            Item::Preset(id) => shared::apply_preset(s, *id, cx),
            Item::Sound(id) => shared::place_sound(s, *id, cx),
            Item::Voice(_) => {}
        });
    }

    fn start_rename(&mut self, id: LibraryId, window: &mut Window, cx: &mut Context<Self>) {
        let lib = &self.state.read(cx).library;
        let Some(name) = lib
            .program(id)
            .map(|p| p.asset.name.clone())
            .or_else(|| lib.preset(id).map(|p| p.name.clone()))
            .or_else(|| lib.sound(id).map(|s| s.name.clone()))
            .or_else(|| lib.voice(id).map(|v| v.name.clone()))
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

    /// The Edit panel header's actions while the library shows: a new
    /// program, and importing into the library.
    pub fn header_actions(&self, cx: &mut Context<Self>) -> AnyElement {
        let state = self.state.clone();
        h_flex()
            .gap(px(2.))
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
            .child(
                Button::new("import-library")
                    .ghost()
                    .small()
                    .icon(IconName::FolderDown)
                    .tooltip("Import to Library")
                    .on_click(cx.listener(|this, _, _, cx| this.import(cx))),
            )
            .into_any_element()
    }

    /// The entries of `section` that match `query`.
    fn shown(&self, section: Section, query: &str, cx: &App) -> Vec<Entry> {
        let state = self.state.read(cx);
        let matches = |e: &Entry| {
            query.is_empty()
                || [e.name.as_ref(), e.category.as_str(), e.description.as_ref()]
                    .iter()
                    .any(|s| s.to_lowercase().contains(query))
        };
        entries(state.bus.project(), &state.library, section)
            .into_iter()
            .filter(matches)
            .collect()
    }

    /// All, then a page for each section that has something to show.
    fn tabs(&self, sections: &[Section], cx: &mut Context<Self>) -> Stateful<Div> {
        let selected = self
            .page
            .and_then(|p| sections.iter().position(|s| *s == p))
            .map_or(0, |ix| ix + 1);
        let pages: Vec<Option<Section>> = std::iter::once(None)
            .chain(sections.iter().copied().map(Some))
            .collect();
        div()
            .id("library-tabs")
            .flex_none()
            .overflow_x_scroll()
            .border_b_1()
            .border_color(t::LN)
            .child(
                Tabs::new("library-pages")
                    .border_b_0()
                    .px(px(10.))
                    .child(Tab::new("All"))
                    .children(sections.iter().map(|s| Tab::new(s.tab())))
                    .selected(selected)
                    .on_click(cx.listener(move |this, ix: &usize, _, cx| {
                        this.page = pages.get(*ix).copied().flatten();
                        cx.notify();
                    })),
            )
    }

    /// `section`'s items under their headings: tiles for what changes the
    /// picture, rows for the rest.
    fn section(
        &self,
        section: Section,
        shown: Vec<Entry>,
        all: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let width = self.width.get().as_f32();
        let cols = if width > 0. {
            ((width - 16. + 8.) / (TILE + 8.)).floor().max(2.) as u16
        } else {
            3
        };
        let mut body = v_flex();
        for (category, list) in grouped(shown) {
            let title = match (all, category.is_empty()) {
                (true, true) => section.label().to_owned(),
                (true, false) => format!("{} · {category}", section.label()),
                (false, true) => String::new(),
                (false, false) => category,
            };
            if !title.is_empty() {
                body = body.child(
                    div()
                        .pt(px(10.))
                        .pb(px(6.))
                        .text_size(px(10.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(t::T3)
                        .child(title),
                );
            }
            let tiles = list.iter().all(|e| e.kind.is_some());
            if tiles {
                body = body.child(
                    div()
                        .grid()
                        .grid_cols(cols)
                        .gap(px(8.))
                        .children(list.into_iter().map(|e| self.tile(e, cx))),
                );
            } else {
                body = body.child(
                    v_flex()
                        .mx(px(-4.))
                        .children(list.into_iter().map(|e| self.row(e, cx))),
                );
            }
        }
        body
    }

    fn element_id(item: &Item) -> SharedString {
        match item {
            Item::Fx(r) => format!("fx-{r}"),
            Item::Audio(k) => format!("afx-{}", k.name()),
            Item::Program(id) => format!("lib-{}", id.0),
            Item::Preset(id) => format!("preset-{}", id.0),
            Item::Sound(id) => format!("sound-{}", id.0),
            Item::Voice(id) => format!("voice-{}", id.0),
        }
        .into()
    }

    /// The library item `item` is being renamed, if it is.
    fn renaming(&self, item: &Item) -> bool {
        match item {
            Item::Program(id) | Item::Preset(id) | Item::Sound(id) | Item::Voice(id) => {
                self.renaming == Some(*id)
            }
            _ => false,
        }
    }

    /// An effect, transition, generator or title as a tile.
    fn tile(&self, e: Entry, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.selected.as_ref() == Some(&e.item);
        let renaming = self.renaming(&e.item);
        let thumb = div()
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .w_full()
            .aspect_ratio(16. / 9.)
            .rounded(t::RADIUS_SM)
            .bg(t::R1)
            .when(selected, |d| d.shadow(ring(t::AC, 1.5)))
            .when(!selected, |d| {
                d.group_hover(Self::element_id(&e.item), |s| {
                    s.shadow(ring(t::alpha(t::T1, 0.13), 1.))
                })
            })
            .child(fx_glyph(
                e.glyph,
                px(18.),
                if selected { t::T1 } else { t::T2 },
            ));
        let name: AnyElement = if renaming {
            div()
                .mt(px(4.))
                .child(Input::new(&self.name).small())
                .into_any_element()
        } else {
            div()
                .mt(px(4.))
                .text_size(px(11.))
                .truncate()
                .text_color(if selected { t::T1 } else { t::T2 })
                .child(e.name.clone())
                .into_any_element()
        };
        let el = self
            .frame(&e, cx)
            .group(Self::element_id(&e.item))
            .flex()
            .flex_col()
            .min_w_0()
            .child(thumb)
            .child(name);
        self.finish(el, &e, cx)
    }

    /// An audio effect, preset, sound or voice as a row.
    fn row(&self, e: Entry, cx: &mut Context<Self>) -> AnyElement {
        if self.renaming(&e.item) {
            return h_flex()
                .h(px(28.))
                .px(px(8.))
                .child(Input::new(&self.name).small())
                .into_any_element();
        }
        let selected = self.selected.as_ref() == Some(&e.item);
        let state = self.state.read(cx);
        let sample = match &e.item {
            Item::Sound(id) => state.library.sound(*id).map(Sound::item),
            Item::Voice(id) => state.library.voice(*id).and_then(SavedVoice::sample_item),
            _ => None,
        };
        let playing = sample.as_ref().and_then(|s| {
            let source = state
                .source
                .as_ref()
                .filter(|p| p.is(s) && p.is_playing())?;
            Some(source.fraction())
        });
        // Gone from the account: it reads nothing until made again.
        let gone = match &e.item {
            Item::Voice(id) => state
                .library
                .voice(*id)
                .is_some_and(|v| state.account_voices.has(&v.voice) == Some(false)),
            _ => false,
        };
        let lead: AnyElement = match &sample {
            _ if gone => div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(20.))
                .rounded_full()
                .border_1()
                .border_color(t::LN)
                .text_color(t::T2)
                .child(Icon::new(IconName::Unlink).size(px(9.)))
                .into_any_element(),
            Some(item) => listen_dot(
                format!("listen-{}", Self::element_id(&e.item)),
                &self.state,
                item,
                px(20.),
                cx,
            )
            .into_any_element(),
            None => div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(20.))
                .child(fx_glyph(
                    e.glyph,
                    px(13.),
                    if selected { t::T1 } else { t::T2 },
                ))
                .into_any_element(),
        };
        let meta = if gone {
            Some(SharedString::from("Not on the account"))
        } else {
            e.meta.clone()
        };
        let el = self
            .frame(&e, cx)
            .h(px(28.))
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(8.))
            .rounded(t::RADIUS)
            .text_color(if selected || playing.is_some() {
                t::T1
            } else {
                t::T2
            })
            .when(selected, |d| d.bg(t::HV))
            .when(!selected, |d| d.hover(|s| s.bg(t::HV).text_color(t::T1)))
            .when(gone, |d| d.opacity(0.45))
            .child(lead)
            .child(div().flex_1().min_w_0().truncate().child(e.name.clone()))
            .children(playing.map(|f| bar3(f).w(px(54.)).flex_none()))
            .children(meta.map(|m| {
                div()
                    .flex_none()
                    .max_w(px(120.))
                    .truncate()
                    .when(!gone, |d| d.font_family(t::mono()))
                    .text_size(px(10.))
                    .text_color(if gone { t::T2 } else { t::T3 })
                    .child(m)
            }));
        self.finish(el, &e, cx)
    }

    /// What tiles and rows share: selecting, applying on a double click,
    /// and the description on hover.
    fn frame(&self, e: &Entry, cx: &mut Context<Self>) -> Stateful<Div> {
        let item = e.item.clone();
        let description = e.description.clone();
        div()
            .id(Self::element_id(&e.item))
            .cursor_pointer()
            .when(!description.is_empty(), |d| {
                d.tooltip(Tooltip::text(description))
            })
            .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                this.selected = Some(item.clone());
                if ev.click_count() >= 2 {
                    this.apply(&item, window, cx);
                }
                cx.notify();
            }))
    }

    /// Dragging onto the timeline and the chat, and the right-click menu.
    fn finish(&self, el: Stateful<Div>, e: &Entry, cx: &mut Context<Self>) -> AnyElement {
        let el = match (&e.item, e.kind) {
            (Item::Fx(r), Some(kind)) => el.on_drag(
                FxDrag {
                    fx: r.clone(),
                    kind,
                    name: e.name.clone(),
                    library: None,
                },
                |drag, _, _, cx| cx.new(|_| drag.clone()),
            ),
            (Item::Program(id), Some(kind)) => el.on_drag(
                FxDrag {
                    fx: FxRef::builtin(""),
                    kind,
                    name: e.name.clone(),
                    library: Some(*id),
                },
                |drag, _, _, cx| cx.new(|_| drag.clone()),
            ),
            (Item::Audio(kind), _) => el.on_drag(
                AudioFxDrag {
                    kind: *kind,
                    name: e.name.clone(),
                },
                |drag, _, _, cx| cx.new(|_| drag.clone()),
            ),
            (Item::Sound(id), _) => el.on_drag(
                SoundDrag {
                    sound: *id,
                    name: e.name.clone(),
                },
                |drag, _, _, cx| cx.new(|_| drag.clone()),
            ),
            _ => el,
        };
        let (panel, state, item) = (cx.entity(), self.state.clone(), e.item.clone());
        el.context_menu(move |menu, _, cx| item_menu(menu, &panel, &state, &item, cx))
            .into_any_element()
    }
}

/// A ring `width` wide around a tile's picture.
fn ring(color: Rgba, width: f32) -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: color.into(),
        offset: point(px(0.), px(0.)),
        blur_radius: px(0.),
        spread_radius: px(width),
        inset: false,
    }]
}

/// How far a sound plays, as a 3 px bar.
fn bar3(fraction: f32) -> Div {
    div()
        .h(px(3.))
        .rounded_full()
        .bg(t::alpha(t::T1, 0.08))
        .child(
            div()
                .h_full()
                .rounded_full()
                .w(relative(fraction.clamp(0., 1.)))
                .bg(t::T1),
        )
}

fn item_menu(
    menu: PopupMenu,
    panel: &Entity<LibraryPanel>,
    state: &Entity<EditorState>,
    item: &Item,
    cx: &App,
) -> PopupMenu {
    if let Item::Voice(id) = item {
        return voice_menu(menu, panel, state, *id, cx);
    }
    let (p, it) = (panel.clone(), item.clone());
    let sound = matches!(item, Item::Sound(_));
    let menu = menu.item(
        PopupMenuItem::new(if sound { "Add to Timeline" } else { "Apply" })
            .on_click(move |_, window, cx| p.update(cx, |this, cx| this.apply(&it, window, cx))),
    );
    let r = match item {
        Item::Fx(r) => r,
        Item::Audio(_) | Item::Voice(_) => return menu,
        Item::Program(id) => return library_menu(menu, panel, state, *id, true),
        Item::Preset(id) => return library_menu(menu, panel, state, *id, false),
        Item::Sound(id) => {
            let (id, s1, s2) = (*id, state.clone(), state.clone());
            let s = state.read(cx);
            let sound = s.library.sound(id).map(Sound::item);
            let preview = s.source.as_ref();
            let playing = sound
                .zip(preview)
                .is_some_and(|(sound, p)| p.is(&sound) && p.is_playing());
            let menu = menu
                .item(
                    PopupMenuItem::new("Add to Project").on_click(move |_, _, cx| {
                        s1.update(cx, |s, cx| {
                            shared::use_sound(s, id, cx);
                        })
                    }),
                )
                .item(
                    PopupMenuItem::new(if playing { "Pause" } else { "Play" }).on_click(
                        move |_, _, cx| {
                            s2.update(cx, |s, cx| {
                                if let Some(sound) = s.library.sound(id).map(Sound::item) {
                                    s.toggle_source(&sound, cx);
                                }
                            })
                        },
                    ),
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

/// Open the voiceover form on library voice `id`, read as it last read.
fn read_with(state: &Entity<EditorState>, id: LibraryId, window: &mut Window, cx: &mut App) {
    let Some(saved) = state.read(cx).library.voice(id).cloned() else {
        return;
    };
    state.update(cx, |s, _| s.remember_voice(&saved.voice));
    let take = crate::ui::media_pool::voiceover::Take {
        script: String::new(),
        reading: saved.reading(),
    };
    crate::ui::media_pool::voiceover::open(state.clone(), Some(take), window, cx);
}

/// What a voice in the library offers: reading with it, hearing it,
/// designing it again when the account no longer has it, renaming,
/// removing it from the library, and deleting it from the account.
fn voice_menu(
    menu: PopupMenu,
    panel: &Entity<LibraryPanel>,
    state: &Entity<EditorState>,
    id: LibraryId,
    cx: &App,
) -> PopupMenu {
    let s = state.read(cx);
    let Some(saved) = s.library.voice(id).cloned() else {
        return menu;
    };
    let gone = s.account_voices.has(&saved.voice) == Some(false);
    let sample = saved.sample_item();
    let playing = sample
        .as_ref()
        .zip(s.source.as_ref())
        .is_some_and(|(item, p)| p.is(item) && p.is_playing());
    let provider = flikk_cloud::provider(&saved.provider).map_or("the provider", |p| p.name);
    let (s1, s2, s3) = (state.clone(), state.clone(), state.clone());
    let mut menu = menu.item(
        PopupMenuItem::new("Read a Voiceover…")
            .disabled(gone)
            .on_click(move |_, window, cx| read_with(&s1, id, window, cx)),
    );
    if let Some(sample) = sample {
        menu = menu.item(
            PopupMenuItem::new(if playing { "Pause" } else { "Play" })
                .on_click(move |_, _, cx| s2.update(cx, |s, cx| s.toggle_source(&sample, cx))),
        );
    }
    if let (true, VoiceOrigin::Designed { description, .. }) = (gone, &saved.origin) {
        let description = description.clone();
        menu = menu.item(
            PopupMenuItem::new("Design Again…").on_click(move |_, window, cx| {
                crate::ui::voices::open(
                    s3.clone(),
                    crate::ui::voices::Start::Describe {
                        description: description.clone(),
                    },
                    None,
                    window,
                    cx,
                )
            }),
        );
    }
    let p = panel.clone();
    let (s4, s5) = (state.clone(), state.clone());
    let name = saved.name.clone();
    menu.item(PopupMenuItem::separator())
        .item(PopupMenuItem::new("Rename").on_click(move |_, window, cx| {
            p.update(cx, |this, cx| this.start_rename(id, window, cx))
        }))
        .item(PopupMenuItem::separator())
        .item(
            PopupMenuItem::new("Remove from Library")
                .on_click(move |_, _, cx| s4.update(cx, |s, cx| s.forget_voice(id, cx))),
        )
        .item(
            PopupMenuItem::new(format!("Delete from {provider}…")).on_click(
                move |_, window, cx| {
                    let (state, name) = (s5.clone(), name.clone());
                    window.open_confirm(cx, move |dialog, _, _| {
                        let state = state.clone();
                        dialog
                            .title(format!("Delete “{name}” from {provider}?"))
                            .description("Voiceovers it read stay.")
                            .destructive()
                            .confirm("Delete", move |_, cx| {
                                state.update(cx, |s, cx| s.delete_saved_voice(id, cx));
                                true
                            })
                    });
                },
            ),
        )
}

/// What a library item offers besides using it.
fn library_menu(
    menu: PopupMenu,
    panel: &Entity<LibraryPanel>,
    state: &Entity<EditorState>,
    id: LibraryId,
    program: bool,
) -> PopupMenu {
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
        let shown: Vec<(Section, Vec<Entry>)> = Section::ALL
            .into_iter()
            .map(|s| (s, self.shown(s, &query, cx)))
            .filter(|(_, e)| !e.is_empty())
            .collect();
        let sections: Vec<Section> = shown.iter().map(|(s, _)| *s).collect();
        if self.page.is_some_and(|p| !sections.contains(&p)) {
            self.page = None;
        }
        let page = self.page;
        let tabs = self.tabs(&sections, cx);
        let empty = shown.is_empty();
        let body: Vec<Div> = shown
            .into_iter()
            .filter(|(s, _)| page.is_none_or(|p| p == *s))
            .map(|(s, entries)| self.section(s, entries, page.is_none(), cx))
            .collect();
        let measured = self.width.clone();
        v_flex()
            .size_full()
            .text_size(px(12.))
            .text_color(t::T1)
            .child(
                div()
                    .flex_none()
                    .px(px(8.))
                    .pt(px(8.))
                    .pb(px(6.))
                    .child(Input::new(&self.search).search()),
            )
            .child(tabs)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        canvas(move |b, _, _| measured.set(b.size.width), |_, _, _, _| {})
                            .absolute()
                            .size_full(),
                    )
                    .child(
                        div()
                            .id("library-items")
                            .size_full()
                            .overflow_y_scrollbar()
                            .when(empty, |d| {
                                d.child(EmptyState::new(IconName::Search, "No matches").compact())
                            })
                            .child(v_flex().px(px(8.)).pb(px(8.)).children(body)),
                    ),
            )
    }
}
