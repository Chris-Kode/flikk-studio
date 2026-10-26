//! Media pool (PRD M1–M4): import, bins, search, tags, thumbnails, analysis
//! progress, source info, and dragging items onto the timeline.

pub(crate) mod demo;
mod import;
mod thumbs;

use crate::state::EditorState;
use crate::ui::MediaDrag;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::command::Command;
use koo_core::model::{Bin, MediaItem, MediaKind};
use koo_core::services::{JobState, JobStatus};
use koo_core::{BinId, MediaId, Time};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use thumbs::Thumbs;

const POLL: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Layout {
    Grid,
    List,
}

pub struct MediaPoolPanel {
    state: Entity<EditorState>,
    search: Entity<InputState>,
    tag_input: Entity<InputState>,
    bin_input: Entity<InputState>,
    layout: Layout,
    /// `None` shows every item.
    bin: Option<BinId>,
    renaming: Option<BinId>,
    selected: Option<MediaId>,
    thumbs: Thumbs,
    jobs: Vec<JobStatus>,
    /// Proxy creation progress, written from background threads.
    proxies: Arc<Mutex<HashMap<MediaId, f32>>>,
    _subscriptions: Vec<Subscription>,
    _poll: Task<()>,
}

impl MediaPoolPanel {
    pub fn new(state: Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        if demo::enabled() {
            state.update(cx, |s, cx| {
                demo::seed(s);
                cx.notify();
            });
        }
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        let tag_input = cx.new(|cx| InputState::new(window, cx).placeholder("Add tag"));
        let bin_input = cx.new(|cx| InputState::new(window, cx));
        let _subscriptions = vec![
            cx.observe(&state, |_, _, cx| cx.notify()),
            cx.subscribe(&search, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe_in(&tag_input, window, |this, input, ev, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    let tag = input.read(cx).value().trim().to_owned();
                    input.update(cx, |i, cx| i.set_value("", window, cx));
                    this.add_tag(tag, cx);
                }
            }),
            cx.subscribe_in(&bin_input, window, |this, input, ev, _, cx| {
                if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    let name = input.read(cx).value().trim().to_owned();
                    this.finish_rename(name, cx);
                }
            }),
        ];
        let _poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                if this.update(cx, |this, cx| this.poll(cx)).is_err() {
                    break;
                }
            }
        });
        let selected = demo::enabled()
            .then(|| state.read(cx).bus.project().media.first().map(|m| m.id))
            .flatten();
        Self {
            state,
            search,
            tag_input,
            bin_input,
            layout: Layout::Grid,
            bin: None,
            renaming: None,
            selected,
            thumbs: Thumbs::default(),
            jobs: Vec::new(),
            proxies: Arc::default(),
            _subscriptions,
            _poll,
        }
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        let jobs = self
            .state
            .read(cx)
            .services
            .analysis
            .as_ref()
            .map(|a| a.jobs())
            .unwrap_or_default();
        let proxying = !self.proxies.lock().unwrap().is_empty();
        if jobs != self.jobs || proxying {
            self.jobs = jobs;
            cx.notify();
        }
    }

    /// Progress of the item's background work, `None` when idle.
    fn progress(&self, id: MediaId) -> Option<Progress> {
        if let Some(f) = self.proxies.lock().unwrap().get(&id) {
            return Some(Progress::Running(*f));
        }
        let job = self.jobs.iter().rev().find(|j| j.media == Some(id))?;
        match &job.state {
            JobState::Queued => Some(Progress::Queued),
            JobState::Running { fraction } => Some(Progress::Running(*fraction)),
            JobState::Done => None,
            JobState::Failed { .. } => Some(Progress::Failed),
        }
    }

    // ------------------------------------------------------------ actions

    fn prompt_import(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                this.update(cx, |this, cx| this.import(paths, cx)).ok();
            }
        })
        .detach();
    }

    fn select(&mut self, id: MediaId, open: bool, cx: &mut Context<Self>) {
        self.selected = Some(id);
        if open {
            self.state
                .update(cx, |s, cx| s.set_source_media(Some(id), cx));
        }
        cx.notify();
    }

    fn update_media(&self, item: MediaItem, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.execute(Command::UpdateMedia { media: item }, cx);
        });
    }

    fn selected_item(&self, cx: &App) -> Option<MediaItem> {
        let id = self.selected?;
        self.state.read(cx).bus.project().media(id).cloned()
    }

    fn add_tag(&mut self, tag: String, cx: &mut Context<Self>) {
        let Some(mut item) = self.selected_item(cx) else {
            return;
        };
        if tag.is_empty() || item.tags.iter().any(|t| t.eq_ignore_ascii_case(&tag)) {
            return;
        }
        item.tags.push(tag);
        self.update_media(item, cx);
    }

    fn remove_tag(&mut self, tag: &str, cx: &mut Context<Self>) {
        let Some(mut item) = self.selected_item(cx) else {
            return;
        };
        item.tags.retain(|t| t != tag);
        self.update_media(item, cx);
    }

    fn move_to_bin(&mut self, media: MediaId, bin: Option<BinId>, cx: &mut Context<Self>) {
        let Some(mut item) = self.state.read(cx).bus.project().media(media).cloned() else {
            return;
        };
        if item.bin != bin {
            item.bin = bin;
            self.update_media(item, cx);
        }
    }

    fn bins(&self, cx: &App) -> Vec<Bin> {
        let state = self.state.read(cx);
        let mut bins = state.bus.project().bins.clone();
        for b in &state.pool_bins {
            if !bins.iter().any(|x| x.id == b.id) {
                bins.push(b.clone());
            }
        }
        bins
    }

    fn new_bin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.bins(cx).len() + 1;
        let bin = Bin {
            id: BinId::new(),
            name: format!("Bin {n}"),
            parent: None,
        };
        let id = bin.id;
        self.state.update(cx, |s, cx| {
            s.pool_bins.push(bin);
            cx.notify();
        });
        self.start_rename(id, window, cx);
    }

    fn start_rename(&mut self, id: BinId, window: &mut Window, cx: &mut Context<Self>) {
        let name = self
            .bins(cx)
            .into_iter()
            .find(|b| b.id == id)
            .map(|b| b.name)
            .unwrap_or_default();
        self.renaming = Some(id);
        // Typing replaces the name; an empty entry keeps it.
        self.bin_input.update(cx, |i, cx| {
            i.set_value("", window, cx);
            i.set_placeholder(name, window, cx);
            i.focus(window, cx);
        });
        cx.notify();
    }

    fn finish_rename(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(id) = self.renaming.take() else {
            return;
        };
        if !name.is_empty() {
            self.state.update(cx, |s, cx| {
                if let Some(b) = s.pool_bins.iter_mut().find(|b| b.id == id) {
                    b.name = name;
                }
                cx.notify();
            });
        }
        cx.notify();
    }

    // ------------------------------------------------------------- render

    fn visible_items(&self, cx: &App) -> Vec<MediaItem> {
        let query = self.search.read(cx).value().trim().to_lowercase();
        let project = self.state.read(cx).bus.project();
        project
            .media
            .iter()
            .filter(|m| self.bin.is_none() || m.bin == self.bin)
            .filter(|m| {
                query.is_empty()
                    || item_name(m).to_lowercase().contains(&query)
                    || m.tags.iter().any(|t| t.to_lowercase().contains(&query))
            })
            .cloned()
            .collect()
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let layout = self.layout;
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
                Button::new("grid")
                    .ghost()
                    .small()
                    .icon(IconName::LayoutDashboard)
                    .tooltip("Grid")
                    .toggled(layout == Layout::Grid)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.layout = Layout::Grid;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("list")
                    .ghost()
                    .small()
                    .icon(IconName::Menu)
                    .tooltip("List")
                    .toggled(layout == Layout::List)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.layout = Layout::List;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("import")
                    .ghost()
                    .small()
                    .icon(IconName::Plus)
                    .tooltip("Import")
                    .on_click(cx.listener(|this, _, _, cx| this.prompt_import(cx))),
            )
    }

    fn bin_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let bins = self.bins(cx);
        let theme = cx.theme();
        let (active, hover, muted, drop) = (
            theme.secondary,
            theme.secondary_hover,
            theme.muted_foreground,
            theme.drop_target,
        );
        let fg = theme.foreground;
        let chip = |id: ElementId, selected: bool| {
            div()
                .id(id)
                .flex_none()
                .px_2()
                .h(px(22.))
                .flex()
                .items_center()
                .rounded_md()
                .text_xs()
                .cursor_pointer()
                .when(selected, |d| d.bg(active).text_color(fg))
                .when(!selected, |d| d.text_color(muted))
                .hover(move |s| s.bg(hover))
                .drag_over::<MediaDrag>(move |s, _, _, _| s.bg(drop))
        };
        let mut row = h_flex().gap_1().child(
            chip("bin-all".into(), self.bin.is_none())
                .child("All")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.bin = None;
                    cx.notify();
                }))
                .on_drop(cx.listener(|this, drag: &MediaDrag, _, cx| {
                    this.move_to_bin(drag.media, None, cx)
                })),
        );
        for bin in bins {
            let id = bin.id;
            if self.renaming == Some(id) {
                row = row.child(div().w(px(96.)).child(Input::new(&self.bin_input).xsmall()));
                continue;
            }
            row = row.child(
                chip(
                    ElementId::Name(format!("bin-{id}").into()),
                    self.bin == Some(id),
                )
                .child(bin.name.clone())
                .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                    if ev.click_count() >= 2 {
                        this.start_rename(id, window, cx);
                    } else {
                        this.bin = Some(id);
                        cx.notify();
                    }
                }))
                .on_drop(cx.listener(move |this, drag: &MediaDrag, _, cx| {
                    this.move_to_bin(drag.media, Some(id), cx)
                })),
            );
        }
        div().id("bins").px_2().pb_1().overflow_x_scroll().child(
            row.child(
                Button::new("new-bin")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Plus)
                    .tooltip("New bin")
                    .on_click(cx.listener(|this, _, window, cx| this.new_bin(window, cx))),
            ),
        )
    }

    fn item(&mut self, item: &MediaItem, cx: &mut Context<Self>) -> AnyElement {
        let thumb = self.thumbs.get(item, &self.state, cx);
        let progress = self.progress(item.id);
        let selected = self.selected == Some(item.id);
        let theme = cx.theme();
        let id = item.id;
        let name: SharedString = item_name(item).into();
        let drag = MediaDrag {
            media: id,
            name: name.clone(),
        };
        let thumb_box = |w: Pixels| {
            let h = w * (9. / 16.);
            div()
                .relative()
                .flex_none()
                .w(w)
                .h(h)
                .rounded_sm()
                .overflow_hidden()
                .bg(theme.muted)
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.muted_foreground)
                .map(|d| match (&thumb, item.kind) {
                    (Some(image), _) => d.child(img(image.clone()).size_full()),
                    (None, MediaKind::Audio) => d.child(IconName::Mic),
                    (None, MediaKind::Image) => d.child(IconName::Frame),
                    (None, MediaKind::Video) => d,
                })
                .children(progress.map(|p| progress_bar(p, cx)))
        };
        let base = div()
            .id(ElementId::Name(format!("media-{id}").into()))
            .rounded_md()
            .cursor_pointer()
            .when(selected, |d| d.bg(theme.list_active))
            .when(!selected, |d| d.hover(|s| s.bg(theme.list_hover)))
            .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                this.select(id, ev.click_count() >= 2, cx)
            }))
            .on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.clone()));
        match self.layout {
            Layout::Grid => base
                .w(px(124.))
                .p_1()
                .flex()
                .flex_col()
                .gap_1()
                .child(thumb_box(px(116.)))
                .child(div().text_xs().truncate().child(name))
                .into_any_element(),
            Layout::List => base
                .h(px(32.))
                .px_1()
                .flex()
                .items_center()
                .gap_2()
                .child(thumb_box(px(44.)))
                .child(div().flex_1().min_w_0().text_xs().truncate().child(name))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .children(item.info.as_ref().map(|i| clock(i.duration))),
                )
                .into_any_element(),
        }
    }

    fn details(&self, item: &MediaItem, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let mut lines: Vec<SharedString> = Vec::new();
        if let Some(info) = &item.info {
            let mut head = vec![clock(info.duration)];
            if let Some(v) = &info.video {
                head.push(format!("{}×{}", v.width, v.height));
                head.push(format!("{:.2} fps", v.rate.fps()).replace(".00", ""));
                head.push(v.codec.clone());
            }
            if !info.container.is_empty() {
                head.push(info.container.clone());
            }
            lines.push(head.join(" · ").into());
            for a in &info.audio {
                let mut parts = vec![format!("A{}", a.index)];
                parts.extend(a.language.clone());
                parts.push(format!("{} ch", a.channels));
                parts.push(format!("{} kHz", a.sample_rate / 1000));
                parts.push(a.codec.clone());
                lines.push(parts.join(" · ").into());
            }
        }
        if let Some(fp) = &item.fingerprint {
            lines.push(fp.0.chars().take(16).collect::<String>().into());
        }
        let tags = item.tags.clone();
        v_flex()
            .gap_1()
            .p_2()
            .border_t_1()
            .border_color(theme.border)
            .child(div().text_sm().truncate().child(item_name(item)))
            .children(
                lines
                    .into_iter()
                    .map(|l| div().text_xs().text_color(muted).truncate().child(l)),
            )
            .child(
                h_flex()
                    .gap_1()
                    .flex_wrap()
                    .children(tags.into_iter().enumerate().map(|(i, tag)| {
                        let t = tag.clone();
                        h_flex()
                            .id(("tag", i))
                            .gap_0p5()
                            .pl_1p5()
                            .pr_0p5()
                            .h(px(20.))
                            .rounded_md()
                            .bg(theme.secondary)
                            .text_xs()
                            .child(tag)
                            .child(
                                div()
                                    .id(("untag", i))
                                    .cursor_pointer()
                                    .text_color(muted)
                                    .hover(|s| s.text_color(theme.foreground))
                                    .child(IconName::Close)
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| this.remove_tag(&t, cx)),
                                    ),
                            )
                    }))
                    .child(div().w(px(96.)).child(Input::new(&self.tag_input).xsmall())),
            )
            .into_any_element()
    }
}

impl Render for MediaPoolPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let items = self.visible_items(cx);
        let elements: Vec<AnyElement> = items.iter().map(|m| self.item(m, cx)).collect();
        let selected = self.selected_item(cx);
        let empty = elements.is_empty();
        let theme = cx.theme();
        let (muted, drop) = (theme.muted_foreground, theme.drop_target);
        v_flex()
            .id("media-pool")
            .size_full()
            .drag_over::<ExternalPaths>(move |s, _, _, _| s.bg(drop))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.import(paths.paths().to_vec(), cx)
            }))
            .child(self.toolbar(cx))
            .child(self.bin_bar(cx))
            .child(
                div()
                    .id("media-items")
                    .flex_1()
                    .min_h_0()
                    .px_1()
                    .overflow_y_scrollbar()
                    .when(empty, |d| {
                        d.flex()
                            .items_center()
                            .justify_center()
                            .text_xs()
                            .text_color(muted)
                            .child(IconName::Inbox)
                    })
                    .when(!empty, |d| match self.layout {
                        Layout::Grid => d.child(h_flex().flex_wrap().gap_1().children(elements)),
                        Layout::List => d.child(v_flex().children(elements)),
                    }),
            )
            .children(selected.map(|item| self.details(&item, cx)))
    }
}

#[derive(Clone, Copy)]
enum Progress {
    Queued,
    Running(f32),
    Failed,
}

/// A 2 px bar along the thumbnail's bottom edge.
fn progress_bar(p: Progress, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let (fraction, color) = match p {
        Progress::Queued => (0.0, theme.primary),
        Progress::Running(f) => (f.clamp(0.0, 1.0), theme.primary),
        Progress::Failed => (1.0, theme.red),
    };
    div()
        .absolute()
        .bottom_0()
        .left_0()
        .right_0()
        .h(px(2.))
        .bg(theme.background.opacity(0.6))
        .child(div().h_full().w(relative(fraction)).bg(color))
}

pub(crate) fn item_name(item: &MediaItem) -> String {
    item.path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| item.path.display().to_string())
}

/// `M:SS`, or `H:MM:SS` past an hour.
pub(crate) fn clock(t: Time) -> String {
    let s = t.as_secs_f64().max(0.0).round() as i64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}
