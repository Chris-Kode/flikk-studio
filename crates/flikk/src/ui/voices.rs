//! Voices: find one on the account or in the provider's shared Voice
//! Library, design one from a description, or clone one from recordings.
//! A shared voice is added to the account; a designed or cloned one is made
//! there. Either way it is kept in the user's library (`.flikkvoice`) and the
//! next voiceover starts on it.

use crate::state::EditorState;
use crate::state::voices::{SIMILAR_LIMIT, SPEAKER_LIMIT, recording};
use crate::ui::kit::theme as t;
use crate::ui::kit::{
    Button, Checkbox, DialogExt as _, DropdownMenu as _, EmptyState, Icon, IconName, Input,
    PopupMenuItem, Radio, ScrollableElement as _, Segmented, Spinner, Switch, Tab, Tabs, Textarea,
    Tooltip, h_flex, v_flex,
};
use crate::ui::media_pool::form;
use crate::ui::media_pool::import::is_audio;
use crate::ui::media_pool::{listen_button, play_dot};
use flikk_core::cloud::{
    self, Capability, CloudError, DESIGN_DESCRIPTION, Voice, VoiceClone, VoiceDesign, VoiceOrigin,
    VoicePreview, VoiceQuery,
};
use flikk_core::model::{MediaItem, MediaKind};
use gpui_kit::base::input::{InputEvent, InputState, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

/// Where the dialog opens.
pub enum Start {
    Find,
    /// Shared voices that sound like `recording`, of `label`.
    Similar {
        recording: PathBuf,
        label: String,
    },
    Describe {
        description: String,
    },
    /// The agent's `design`, named `name`, already designing: the user
    /// hears the takes and keeps one.
    Design {
        design: VoiceDesign,
        name: String,
    },
    /// Clone from `files`, named `name`.
    Clone {
        files: Vec<PathBuf>,
        name: String,
    },
}

/// Called with the voice picked, added or made, after the dialog closes.
pub type OnKept = Rc<dyn Fn(&Voice, &mut Window, &mut App)>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Find,
    Describe,
    Clone,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Account,
    Shared,
}

/// What is on its way.
#[derive(Clone, PartialEq, Eq)]
enum Busy {
    Searching,
    Adding(String),
    Designing,
    Saving,
    Cloning,
}

/// A filter of the shared library.
#[derive(Clone, Copy)]
enum Filter {
    Gender,
    Age,
    Language,
    Use,
}

impl Filter {
    const ALL: [Filter; 4] = [Self::Gender, Self::Age, Self::Language, Self::Use];

    fn label(self) -> &'static str {
        match self {
            Self::Gender => "Gender",
            Self::Age => "Age",
            Self::Language => "Language",
            Self::Use => "Use",
        }
    }

    /// Its values, as sent and as shown.
    fn values(self) -> Vec<(&'static str, String)> {
        let labelled = |values: &[&'static str]| {
            values
                .iter()
                .map(|v| (*v, cloud::voice_label(v)))
                .collect::<Vec<_>>()
        };
        match self {
            Self::Gender => labelled(&cloud::VOICE_GENDERS),
            Self::Age => labelled(&cloud::VOICE_AGES),
            Self::Use => labelled(&cloud::VOICE_USES),
            Self::Language => cloud::VOICE_LANGUAGES
                .iter()
                .map(|(code, name)| (*code, (*name).to_owned()))
                .collect(),
        }
    }

    fn slot(self, query: &mut VoiceQuery) -> &mut Option<String> {
        match self {
            Self::Gender => &mut query.gender,
            Self::Age => &mut query.age,
            Self::Language => &mut query.language,
            Self::Use => &mut query.use_case,
        }
    }

    fn value(self, query: &VoiceQuery) -> Option<&String> {
        match self {
            Self::Gender => query.gender.as_ref(),
            Self::Age => query.age.as_ref(),
            Self::Language => query.language.as_ref(),
            Self::Use => query.use_case.as_ref(),
        }
    }
}

pub struct VoicesDialog {
    state: Entity<EditorState>,
    on_kept: Option<OnKept>,
    page: Page,
    busy: Option<Busy>,
    error: Option<SharedString>,
    // Find
    scope: Scope,
    search: Entity<InputState>,
    query: VoiceQuery,
    shared: Vec<Voice>,
    shared_page: usize,
    more: bool,
    /// Searched since the query last changed.
    searched: bool,
    /// The recording shared voices were matched to, and whose it is.
    similar: Option<(PathBuf, String)>,
    selected: Option<String>,
    samples: HashMap<String, MediaItem>,
    fetching: Option<String>,
    // Describe
    name: Entity<InputState>,
    description: Entity<TextareaState>,
    sample_text: Entity<TextareaState>,
    /// The design the previews are of.
    designed: Option<VoiceDesign>,
    previews: Vec<(VoicePreview, MediaItem)>,
    picked: Option<usize>,
    // From audio
    clone_name: Entity<InputState>,
    files: Vec<PathBuf>,
    denoise: bool,
    consent: bool,
    _subscriptions: Vec<Subscription>,
}

impl VoicesDialog {
    fn new(
        state: Entity<EditorState>,
        start: Start,
        on_kept: Option<OnKept>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search voices"));
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Name"));
        let clone_name = cx.new(|cx| InputState::new(window, cx).placeholder("Name"));
        let description = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(3, 6)
                .placeholder("A warm, slightly raspy narrator in his late 40s, British, unhurried…")
        });
        let sample_text = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 4)
                .placeholder("What the samples say (optional)")
        });
        let _subscriptions = vec![
            cx.observe(&state, |_, _, cx| cx.notify()),
            cx.subscribe(&search, |this, _, ev: &InputEvent, cx| match ev {
                InputEvent::PressEnter { .. } if this.scope == Scope::Shared => {
                    this.similar = None;
                    this.search_shared(0, cx);
                }
                InputEvent::Change => {
                    this.searched = false;
                    cx.notify();
                }
                _ => {}
            }),
            cx.subscribe(&name, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe(&clone_name, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe(&description, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe(&sample_text, |_, _, _: &InputEvent, cx| cx.notify()),
        ];
        state.update(cx, |s, cx| s.want_voices(cx));
        let mut this = Self {
            state,
            on_kept,
            page: Page::Find,
            busy: None,
            error: None,
            scope: Scope::Account,
            search,
            query: VoiceQuery::default(),
            shared: Vec::new(),
            shared_page: 0,
            more: false,
            searched: false,
            similar: None,
            selected: None,
            samples: HashMap::new(),
            fetching: None,
            name,
            description,
            sample_text,
            designed: None,
            previews: Vec::new(),
            picked: None,
            clone_name,
            files: Vec::new(),
            denoise: false,
            consent: false,
            _subscriptions,
        };
        match start {
            Start::Find => {}
            Start::Similar { recording, label } => {
                this.scope = Scope::Shared;
                this.similar = Some((recording, label));
                this.find_similar(cx);
            }
            Start::Describe { description } => {
                this.page = Page::Describe;
                this.description
                    .update(cx, |d, cx| d.set_value(description, window, cx));
            }
            Start::Design { design, name } => {
                this.page = Page::Describe;
                this.name.update(cx, |n, cx| n.set_value(name, window, cx));
                this.description
                    .update(cx, |d, cx| d.set_value(design.description, window, cx));
                if let Some(text) = design.text {
                    this.sample_text
                        .update(cx, |t, cx| t.set_value(text, window, cx));
                }
                cx.defer_in(window, |this, window, cx| this.design(window, cx));
            }
            Start::Clone { files, name } => {
                this.page = Page::Clone;
                this.files = files;
                this.clone_name
                    .update(cx, |n, cx| n.set_value(name, window, cx));
            }
        }
        this
    }

    fn fail(&mut self, e: impl ToString, cx: &mut Context<Self>) {
        self.busy = None;
        self.error = Some(e.to_string().into());
        cx.notify();
    }

    /// Run `work` off the UI thread as `busy`, then `done` with its result.
    fn run<T: Send + 'static>(
        &mut self,
        busy: Busy,
        work: impl FnOnce() -> Result<T, String> + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.busy = Some(busy);
        self.error = None;
        let task = cx.background_executor().spawn(async move { work() });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = None;
                match result {
                    Ok(value) => done(this, value, window, cx),
                    Err(e) => this.fail(e, cx),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Close, and hand `voice` on.
    fn kept(&mut self, voice: Voice, window: &mut Window, cx: &mut Context<Self>) {
        window.close_dialog(cx);
        if let Some(on_kept) = self.on_kept.clone() {
            on_kept(&voice, window, cx);
        }
    }

    // ---------------------------------------------------------------- find

    /// The account's voices that fit the query, the user's own first.
    fn account(&self, cx: &App) -> Vec<Voice> {
        let mut query = self.query.clone();
        query.text = self.search.read(cx).value().to_string();
        let state = self.state.read(cx);
        let mut voices: Vec<Voice> = state
            .account_voices
            .voices
            .iter()
            .flatten()
            .filter(|v| query.fits(v))
            .cloned()
            .collect();
        voices.sort_by_key(|v| !v.mine());
        voices
    }

    fn shown(&self, cx: &App) -> Vec<Voice> {
        match self.scope {
            Scope::Account => self.account(cx),
            Scope::Shared => self.shared.clone(),
        }
    }

    fn selected_voice(&self, cx: &App) -> Option<Voice> {
        let id = self.selected.as_ref()?;
        self.shown(cx).into_iter().find(|v| v.id == *id)
    }

    fn search_shared(&mut self, page: usize, cx: &mut Context<Self>) {
        let Some(generation) = self.state.read(cx).generation() else {
            return;
        };
        let mut query = self.query.clone();
        query.text = self.search.read(cx).value().trim().to_owned();
        self.busy = Some(Busy::Searching);
        self.error = None;
        self.searched = true;
        let task = cx
            .background_executor()
            .spawn(async move { generation.search_voices(&query, page) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = None;
                match result {
                    Ok(found) => {
                        if page == 0 {
                            this.shared = found.voices;
                        } else {
                            this.shared.extend(found.voices);
                        }
                        this.shared_page = page;
                        this.more = found.more;
                    }
                    Err(e) => this.fail(e, cx),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn find_similar(&mut self, cx: &mut Context<Self>) {
        let Some((recording, _)) = self.similar.clone() else {
            return;
        };
        let Some(generation) = self.state.read(cx).generation() else {
            return;
        };
        self.busy = Some(Busy::Searching);
        self.error = None;
        self.searched = true;
        self.more = false;
        let task = cx.background_executor().spawn(async move {
            let clip = recording_clip(&recording)?;
            generation.similar_voices(&clip).map_err(|e| e.to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = None;
                match result {
                    Ok(voices) => this.shared = voices,
                    Err(e) => this.fail(e, cx),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn set_scope(&mut self, scope: Scope, cx: &mut Context<Self>) {
        self.scope = scope;
        self.selected = None;
        if scope == Scope::Shared && self.shared.is_empty() && self.busy.is_none() {
            self.search_shared(0, cx);
        }
        cx.notify();
    }

    fn set_filter(&mut self, filter: Filter, value: Option<String>, cx: &mut Context<Self>) {
        *filter.slot(&mut self.query) = value;
        self.selected = None;
        if self.scope == Scope::Shared {
            self.similar = None;
            self.search_shared(0, cx);
        }
        cx.notify();
    }

    /// Use the account's `voice`, kept in the library when it is one the
    /// user made.
    fn use_voice(&mut self, voice: Voice, window: &mut Window, cx: &mut Context<Self>) {
        let sample = self.samples.get(&voice.id).cloned();
        self.state.update(cx, |s, cx| {
            let provider = s.voice_provider().unwrap_or_default();
            if voice.mine() && s.library.voice_of(&provider, &voice.id).is_none() {
                s.keep_voice(&voice, VoiceOrigin::Account, sample.as_ref(), cx);
            } else {
                s.remember_voice(&voice.id);
            }
        });
        self.kept(voice, window, cx);
    }

    /// Add shared `voice` to the account, then keep it.
    fn add_voice(&mut self, voice: Voice, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let (Some(generation), media) = (state.generation(), state.services.media.clone()) else {
            return;
        };
        let shared = voice.clone();
        self.run(
            Busy::Adding(voice.id.clone()),
            move || {
                let added = generation
                    .add_voice(&shared, &shared.name)
                    .map_err(|e| e.to_string())?;
                let sample = generation
                    .voice_sample(&shared)
                    .ok()
                    .map(|path| cloud::sample_item(path, media.as_deref()));
                Ok((added, sample))
            },
            move |this, (added, sample), window, cx| {
                let origin = VoiceOrigin::Shared {
                    owner: voice.owner.clone().unwrap_or_default(),
                    name: voice.name.clone(),
                };
                this.state.update(cx, |s, cx| {
                    s.keep_voice(&added, origin, sample.as_ref(), cx);
                });
                this.kept(added, window, cx);
            },
            window,
            cx,
        );
    }

    /// Fetch the sample of `voice`, then play it.
    fn fetch_sample(&mut self, voice: Voice, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let (Some(generation), media) = (state.generation(), state.services.media.clone()) else {
            return;
        };
        let id = voice.id.clone();
        self.fetching = Some(id.clone());
        self.error = None;
        let fetch = cx.background_executor().spawn(async move {
            let path = generation.voice_sample(&voice)?;
            Ok::<_, CloudError>(cloud::sample_item(path, media.as_deref()))
        });
        cx.spawn(async move |this, cx| {
            let result = fetch.await;
            let _ = this.update(cx, |this, cx| {
                this.fetching = None;
                match result {
                    Ok(item) => {
                        this.state.update(cx, |s, cx| s.toggle_source(&item, cx));
                        this.samples.insert(id, item);
                    }
                    Err(e) => this.error = Some(e.to_string().into()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn sample_button(&self, voice: &Voice, cx: &mut Context<Self>) -> AnyElement {
        if self.fetching.as_ref() == Some(&voice.id) {
            return spinner_box().into_any_element();
        }
        let id = SharedString::from(format!("sample-{}", voice.id));
        if let Some(item) = self.samples.get(&voice.id) {
            return listen_button(id, &self.state, item, cx).into_any_element();
        }
        let heard = voice.sample.is_some();
        let voice = voice.clone();
        play_dot(id, false, px(22.))
            .when(!heard, |d| d.opacity(0.4))
            .tooltip(Tooltip::text("Play"))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                if voice.sample.is_some() {
                    this.fetch_sample(voice.clone(), cx);
                }
            }))
            .into_any_element()
    }

    fn voice_row(&self, voice: &Voice, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.selected.as_ref() == Some(&voice.id);
        let adding = self.busy == Some(Busy::Adding(voice.id.clone()));
        let id = voice.id.clone();
        let picked = voice.clone();
        let taken = voice.clone();
        let rate = voice.rate.map(|r| format!("×{r:.1}"));
        let notice = voice.notice_days;
        let row_id = SharedString::from(format!("voice-{}", voice.id));
        let tags = voice.traits.iter().take(2).map(|tag| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .h(px(18.))
                .px(px(6.))
                .rounded(t::RADIUS_SM)
                .bg(t::alpha(t::T1, 0.04))
                .text_size(px(10.))
                .text_color(t::T2)
                .child(cloud::voice_label(tag))
        });
        let action = Button::new(SharedString::from(format!("take-{}", voice.id)))
            .small()
            .label(if voice.shared() { "Add" } else { "Use" })
            .loading(adding)
            .disabled(self.busy.is_some() && !adding)
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                if this.busy.is_none() {
                    this.take(taken.clone(), window, cx);
                }
            }));
        let action = if selected { action.primary() } else { action };
        h_flex()
            .id(row_id)
            .h(px(40.))
            .px(px(10.))
            .gap(px(10.))
            .rounded(t::RADIUS)
            .cursor_pointer()
            .when(selected, |d| {
                d.bg(t::alpha(t::AC, 0.08))
                    .shadow(inset_ring(t::alpha(t::AC, 0.4)))
            })
            .when(!selected, |d| d.hover(|s| s.bg(t::HV)))
            .child(self.sample_button(voice, cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().truncate().child(voice.name.clone()))
                    .when(!voice.description.is_empty(), |d| {
                        d.child(
                            div()
                                .truncate()
                                .text_size(px(10.))
                                .text_color(t::T3)
                                .child(voice.description.clone()),
                        )
                    }),
            )
            .child(h_flex().gap(px(3.)).flex_none().children(tags))
            .when_some(rate, |d, rate| {
                let mut text = "Costs more credits per character".to_owned();
                if let Some(days) = notice {
                    text.push_str(&format!("; can be withdrawn with {days} days' notice"));
                }
                d.child(
                    div()
                        .id(SharedString::from(format!("rate-{}", voice.id)))
                        .flex_none()
                        .font_family(t::mono())
                        .text_size(px(10.))
                        .text_color(t::WARN)
                        .child(rate)
                        .tooltip(Tooltip::text(text)),
                )
            })
            .child(action)
            .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                this.selected = Some(id.clone());
                if ev.click_count() >= 2 && this.busy.is_none() {
                    this.take(picked.clone(), window, cx);
                }
                cx.notify();
            }))
            .into_any_element()
    }

    /// Use an account voice, or add a shared one.
    fn take(&mut self, voice: Voice, window: &mut Window, cx: &mut Context<Self>) {
        if voice.shared() {
            self.add_voice(voice, window, cx);
        } else {
            self.use_voice(voice, window, cx);
        }
    }

    fn filter_button(&self, filter: Filter, cx: &mut Context<Self>) -> AnyElement {
        let dialog = cx.entity();
        let current = filter.value(&self.query).cloned();
        let value = current.as_ref().map_or_else(
            || "Any".to_owned(),
            |v| {
                filter
                    .values()
                    .into_iter()
                    .find(|(code, _)| code == v)
                    .map_or_else(|| v.clone(), |(_, name)| name)
            },
        );
        let chip = Button::new(SharedString::from(format!("filter-{}", filter.label())))
            .xsmall()
            .label(format!("{} · {value}", filter.label()))
            .trailing_icon(IconName::ChevronDown)
            .h(px(22.))
            .px(px(8.))
            .rounded_full()
            .text_color(t::T2)
            .when(current.is_some(), |b| {
                b.border_color(t::alpha(t::AC, 0.53))
                    .bg(t::alpha(t::AC, 0.08))
                    .text_color(t::T1)
            });
        chip.dropdown_menu(move |mut menu, _, _| {
            let d = dialog.clone();
            menu = menu.item(PopupMenuItem::new("Any").on_click(move |_, _, cx| {
                d.update(cx, |this, cx| this.set_filter(filter, None, cx))
            }));
            for (code, name) in filter.values() {
                let d = dialog.clone();
                menu = menu.item(PopupMenuItem::new(name).on_click(move |_, _, cx| {
                    d.update(cx, |this, cx| {
                        this.set_filter(filter, Some(code.to_owned()), cx)
                    })
                }));
            }
            menu
        })
        .into_any_element()
    }

    fn find_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let voices = self.shown(cx);
        let state = self.state.read(cx);
        let loading = match self.scope {
            Scope::Account => state.account_voices.loading,
            Scope::Shared => self.busy == Some(Busy::Searching),
        };
        let load_error = match self.scope {
            Scope::Account => state.account_voices.error.clone(),
            Scope::Shared => None,
        };
        let scope = Segmented::new("voice-scope")
            .items(["Account", "Voice Library"])
            .selected(match self.scope {
                Scope::Account => 0,
                Scope::Shared => 1,
            })
            .fill()
            .w(px(220.))
            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                let scope = if *ix == 0 {
                    Scope::Account
                } else {
                    Scope::Shared
                };
                this.set_scope(scope, cx);
            }));
        let similar = self.similar.as_ref().map(|(_, label)| label.clone());
        let filters = h_flex()
            .gap(px(6.))
            .flex_wrap()
            .children(
                Filter::ALL
                    .into_iter()
                    .map(|f| self.filter_button(f, cx))
                    .collect::<Vec<_>>(),
            )
            .when_some(similar, |el, label| {
                el.child(
                    h_flex()
                        .gap(px(4.))
                        .h(px(22.))
                        .pl(px(8.))
                        .pr(px(2.))
                        .rounded_full()
                        .border_1()
                        .border_color(t::LN)
                        .text_size(px(11.))
                        .child(Icon::new(IconName::AudioLines).size(px(11.)).color(t::A))
                        .child(format!("Like {label}"))
                        .child(
                            Button::new("clear-similar")
                                .ghost()
                                .xsmall()
                                .icon(IconName::X)
                                .rounded_full()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.similar = None;
                                    this.search_shared(0, cx);
                                })),
                        ),
                )
            });
        let rows: Vec<AnyElement> = voices.iter().map(|v| self.voice_row(v, cx)).collect();
        let empty = rows.is_empty();
        let more = self.scope == Scope::Shared && self.more && self.similar.is_none();
        let next = self.shared_page + 1;
        let nothing = !loading
            && empty
            && load_error.is_none()
            && self.error.is_none()
            && (self.searched || self.scope == Scope::Account);
        v_flex()
            .gap(px(10.))
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.search).search()),
                    )
                    .child(scope),
            )
            .child(filters)
            .child(
                div()
                    .id("voice-list")
                    .h(px(300.))
                    .mx(px(-10.))
                    .overflow_y_scrollbar()
                    .when(loading && empty, |d| {
                        d.flex()
                            .items_center()
                            .justify_center()
                            .child(Spinner::new().size(px(14.)))
                    })
                    .when_some(load_error.filter(|_| empty), |d, e| {
                        d.p(px(10.)).child(form::problem(e))
                    })
                    .when(nothing, |d| {
                        d.child(EmptyState::new(IconName::Search, "No voices found").compact())
                    })
                    .child(v_flex().gap(px(2.)).children(rows))
                    .when(more, |d| {
                        d.child(
                            h_flex().justify_center().p(px(4.)).child(
                                Button::new("more-voices")
                                    .ghost()
                                    .small()
                                    .label("More")
                                    .loading(loading)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.search_shared(next, cx)
                                    })),
                            ),
                        )
                    }),
            )
            .into_any_element()
    }

    // ------------------------------------------------------------ describe

    fn design_request(&self, cx: &App) -> VoiceDesign {
        let text = self.sample_text.read(cx).value().trim().to_owned();
        VoiceDesign {
            description: self.description.read(cx).value().trim().to_owned(),
            text: (!text.is_empty()).then_some(text),
        }
    }

    /// The previews are of what the form says now.
    fn designed_now(&self, cx: &App) -> bool {
        !self.previews.is_empty() && self.designed.as_ref() == Some(&self.design_request(cx))
    }

    fn design(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let design = self.design_request(cx);
        if let Err(e) = design.check() {
            return self.fail(e, cx);
        }
        let state = self.state.read(cx);
        let (Some(generation), media) = (state.generation(), state.services.media.clone()) else {
            return;
        };
        let asked = design.clone();
        self.run(
            Busy::Designing,
            move || {
                let designed = generation.design_voice(&asked).map_err(|e| e.to_string())?;
                Ok(designed
                    .previews
                    .into_iter()
                    .map(|p| {
                        let item = cloud::sample_item(p.audio.clone(), media.as_deref());
                        (p, item)
                    })
                    .collect::<Vec<_>>())
            },
            move |this, previews, _, cx| {
                this.previews = previews;
                this.picked = Some(0);
                this.designed = Some(design);
                if let Some((_, item)) = this.previews.first() {
                    this.state.update(cx, |s, cx| s.toggle_source(item, cx));
                }
            },
            window,
            cx,
        );
    }

    fn save_design(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(design), Some(i)) = (self.designed.clone(), self.picked) else {
            return;
        };
        let Some((preview, item)) = self.previews.get(i).cloned() else {
            return;
        };
        let name = self.name.read(cx).value().trim().to_owned();
        let Some(generation) = self.state.read(cx).generation() else {
            return;
        };
        let asked = design.clone();
        self.run(
            Busy::Saving,
            move || {
                generation
                    .save_voice(&asked, &preview, &name)
                    .map_err(|e| e.to_string())
            },
            move |this, voice, window, cx| {
                let origin = VoiceOrigin::Designed {
                    description: design.description.clone(),
                    text: design.text.clone(),
                };
                this.state.update(cx, |s, cx| {
                    s.keep_voice(&voice, origin, Some(&item), cx);
                });
                this.kept(voice, window, cx);
            },
            window,
            cx,
        );
    }

    fn describe_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let chars = self.description.read(cx).value().trim().chars().count();
        let (min, max) = DESIGN_DESCRIPTION;
        let current = self.designed_now(cx);
        let idle = self.busy.is_none();
        let previews: Vec<AnyElement> =
            self.previews
                .iter()
                .enumerate()
                .map(|(i, (_, item))| {
                    let picked = self.picked == Some(i) && current;
                    let state = self.state.read(cx);
                    let playing = state
                        .source
                        .as_ref()
                        .filter(|p| p.is(item) && p.is_playing())
                        .map(|p| p.fraction());
                    h_flex()
                        .id(("preview", i))
                        .h(px(30.))
                        .px(px(8.))
                        .gap(px(8.))
                        .rounded(t::RADIUS)
                        .cursor_pointer()
                        .when(picked, |d| d.bg(t::HV))
                        .when(!picked, |d| d.hover(|s| s.bg(t::HV)))
                        .child(listen_button(("listen-preview", i), &self.state, item, cx))
                        .child(
                            div()
                                .flex_1()
                                .text_color(if picked { t::T1 } else { t::T2 })
                                .child(format!("Take {}", i + 1)),
                        )
                        .children(playing.map(|f| bar3(f).w(px(70.)).flex_none()))
                        .child(Radio::new(("pick-preview", i)).checked(picked).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.picked = Some(i);
                                cx.notify();
                            }),
                        ))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.picked = Some(i);
                            cx.notify();
                        }))
                        .into_any_element()
                })
                .collect();
        v_flex()
            .gap(px(10.))
            .child(Textarea::new(&self.description).counter(chars, max))
            .child(Textarea::new(&self.sample_text))
            .child(
                h_flex()
                    .gap(px(6.))
                    .child(div().flex_1().min_w_0().child(Input::new(&self.name)))
                    .child(
                        Button::new("design-voice")
                            .label("Generate")
                            .loading(self.busy == Some(Busy::Designing))
                            .disabled(!idle || chars < min)
                            .on_click(cx.listener(|this, _, window, cx| this.design(window, cx))),
                    ),
            )
            .when(!previews.is_empty(), |el| {
                el.child(
                    v_flex()
                        .gap(px(2.))
                        .mx(px(-6.))
                        .when(!current, |d| d.opacity(0.5))
                        .children(previews),
                )
            })
            .into_any_element()
    }

    // ----------------------------------------------------------- from audio

    fn add_files(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                let _ = this.update(cx, |this, cx| {
                    for path in paths {
                        if !this.files.contains(&path) {
                            this.files.push(path);
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn clone_voice(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.clone_name.read(cx).value().trim().to_owned();
        let files = self.files.clone();
        let denoise = self.denoise;
        let state = self.state.read(cx);
        let (Some(generation), media) = (state.generation(), state.services.media.clone()) else {
            return;
        };
        let sources: Vec<String> = files.iter().map(|p| file_name(p)).collect();
        let made = name.clone();
        self.run(
            Busy::Cloning,
            move || {
                // A video, or a format the provider may not take, goes as
                // its sound.
                let files = files
                    .iter()
                    .map(|p| {
                        if is_audio(p) && is_upload_format(p) {
                            Ok(p.clone())
                        } else {
                            recording(p, &[], SPEAKER_LIMIT, &made)
                        }
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                let voice = generation
                    .clone_voice(&VoiceClone {
                        name: made.clone(),
                        description: String::new(),
                        files: files.clone(),
                        denoise,
                    })
                    .map_err(|e| e.to_string())?;
                let sample = files
                    .first()
                    .map(|p| cloud::sample_item(p.clone(), media.as_deref()));
                Ok((voice, sample))
            },
            move |this, (voice, sample), window, cx| {
                this.state.update(cx, |s, cx| {
                    s.keep_voice(&voice, VoiceOrigin::Cloned { sources }, sample.as_ref(), cx);
                });
                this.kept(voice, window, cx);
            },
            window,
            cx,
        );
    }

    fn clone_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let dialog = cx.entity();
        let pool: Vec<(SharedString, PathBuf)> = self
            .state
            .read(cx)
            .bus
            .project()
            .media
            .iter()
            .filter(|m| matches!(m.kind, MediaKind::Audio | MediaKind::Video))
            .map(|m| (crate::ui::media_pool::item_name(m).into(), m.path.clone()))
            .collect();
        let files: Vec<AnyElement> = self
            .files
            .iter()
            .enumerate()
            .map(|(i, path)| {
                h_flex()
                    .h(px(28.))
                    .px(px(8.))
                    .gap(px(8.))
                    .rounded(t::RADIUS)
                    .child(Icon::new(IconName::AudioLines).size(px(13.)).color(t::A))
                    .child(div().flex_1().min_w_0().truncate().child(file_name(path)))
                    .child(
                        Button::new(("remove-file", i))
                            .ghost()
                            .xsmall()
                            .icon(IconName::X)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.files.remove(i);
                                cx.notify();
                            })),
                    )
                    .into_any_element()
            })
            .collect();
        v_flex()
            .gap(px(10.))
            .child(Input::new(&self.clone_name))
            .when(!files.is_empty(), |el| {
                el.child(
                    v_flex()
                        .p(px(4.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(t::LN)
                        .bg(t::G)
                        .children(files),
                )
            })
            .child(
                h_flex()
                    .gap(px(6.))
                    .child(
                        Button::new("add-files")
                            .small()
                            .label("Add files…")
                            .on_click(cx.listener(|this, _, _, cx| this.add_files(cx))),
                    )
                    .when(!pool.is_empty(), |el| {
                        el.child(
                            Button::new("from-pool")
                                .small()
                                .label("From the pool")
                                .trailing_icon(IconName::ChevronDown)
                                .dropdown_menu(move |mut menu, _, _| {
                                    for (name, path) in &pool {
                                        let (d, path) = (dialog.clone(), path.clone());
                                        menu =
                                            menu.item(PopupMenuItem::new(name.clone()).on_click(
                                                move |_, _, cx| {
                                                    d.update(cx, |this, cx| {
                                                        if !this.files.contains(&path) {
                                                            this.files.push(path.clone());
                                                        }
                                                        cx.notify();
                                                    })
                                                },
                                            ));
                                    }
                                    menu
                                }),
                        )
                    }),
            )
            .child(
                Switch::new("denoise")
                    .label("Remove background noise")
                    .checked(self.denoise)
                    .on_click(cx.listener(|this, on: &bool, _, cx| {
                        this.denoise = *on;
                        cx.notify();
                    })),
            )
            .child(
                Checkbox::new("consent")
                    .label("I have the right to clone this voice")
                    .checked(self.consent)
                    .on_click(cx.listener(|this, on: &bool, _, cx| {
                        this.consent = *on;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    // --------------------------------------------------------------- footer

    /// The footer's button: its label, and whether it can be pressed.
    fn primary(&self, cx: &App) -> (&'static str, bool) {
        let idle = self.busy.is_none();
        match self.page {
            Page::Find => {
                let voice = self.selected_voice(cx);
                let label = if voice.as_ref().is_some_and(Voice::shared) {
                    "Add & use"
                } else {
                    "Use"
                };
                (label, idle && voice.is_some())
            }
            Page::Describe => {
                let named = !self.name.read(cx).value().trim().is_empty();
                let ready = self.designed_now(cx) && named && self.picked.is_some();
                ("Save", idle && ready)
            }
            Page::Clone => {
                let named = !self.clone_name.read(cx).value().trim().is_empty();
                (
                    "Create",
                    idle && named && self.consent && !self.files.is_empty(),
                )
            }
        }
    }

    /// Saving or making what the footer button asked for.
    fn working(&self) -> bool {
        matches!(self.busy, Some(Busy::Saving | Busy::Cloning))
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.primary(cx).1 {
            return;
        }
        match self.page {
            Page::Find => {
                if let Some(voice) = self.selected_voice(cx) {
                    self.take(voice, window, cx);
                }
            }
            Page::Describe => self.save_design(window, cx),
            Page::Clone => self.clone_voice(window, cx),
        }
    }
}

impl Render for VoicesDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = Tabs::new("voices-page")
            .mx(px(-16.))
            .px(px(16.))
            .gap(px(16.))
            .selected(match self.page {
                Page::Find => 0,
                Page::Describe => 1,
                Page::Clone => 2,
            })
            .child(Tab::new("Find"))
            .child(Tab::new("Describe"))
            .child(Tab::new("From audio"))
            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                this.page = match ix {
                    0 => Page::Find,
                    1 => Page::Describe,
                    _ => Page::Clone,
                };
                this.error = None;
                cx.notify();
            }));
        let page = match self.page {
            Page::Find => self.find_page(cx),
            Page::Describe => self.describe_page(cx),
            Page::Clone => self.clone_page(cx),
        };
        v_flex()
            .gap(px(12.))
            .child(tabs)
            .child(page)
            .when_some(self.error.clone(), |el, error| {
                el.child(form::problem(error))
            })
    }
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// A spinner the size of a play button.
fn spinner_box() -> impl IntoElement {
    div()
        .flex()
        .flex_none()
        .size(px(22.))
        .items_center()
        .justify_center()
        .child(Spinner::new())
}

/// How far a take plays, as a 3 px bar.
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

/// A 1 px ring inside a selected row.
fn inset_ring(color: Rgba) -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: color.into(),
        offset: point(px(0.), px(0.)),
        blur_radius: px(0.),
        spread_radius: px(1.),
        inset: true,
    }]
}

/// Formats every provider takes as they are.
fn is_upload_format(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|e| matches!(e.as_str(), "mp3" | "wav" | "flac" | "m4a" | "ogg"))
}

/// The start of `recording`, short enough to send for a match.
fn recording_clip(source: &std::path::Path) -> Result<PathBuf, String> {
    recording(source, &[], SIMILAR_LIMIT, "similar")
}

/// Whether a provider set up for voiceovers can find and make voices.
pub fn available(state: &EditorState) -> bool {
    state
        .services
        .generation
        .as_ref()
        .is_some_and(|g| g.provider_for(Capability::Voiceover).is_some())
}

/// Open the dialog at `start`; `on_kept` hears of the voice chosen.
pub fn open(
    state: Entity<EditorState>,
    start: Start,
    on_kept: Option<OnKept>,
    window: &mut Window,
    cx: &mut App,
) {
    let dialog = cx.new(|cx| VoicesDialog::new(state, start, on_kept, window, cx));
    window.open_dialog(cx, move |d, _, cx| {
        let submit = dialog.clone();
        let (label, ready) = dialog.read(cx).primary(cx);
        let working = dialog.read(cx).working();
        d.title("Voices")
            .w(px(580.))
            .child(dialog.clone())
            .confirm(label, move |window, cx| {
                submit.update(cx, |f, cx| f.submit(window, cx));
                false
            })
            .confirm_disabled(!ready)
            .working(working)
    });
}
