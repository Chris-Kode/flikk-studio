//! Settings → Storage: what the user-data folder holds, by kind, each with
//! its folder to open, and a way to clear what can be made again; each
//! row clears what it counts. Models are deleted one by one on the Models
//! page. The render cache's row also turns background rendering on and
//! off and sets its size limit.

use super::{meta, page, row, size_label};
use crate::render_cache;
use crate::ui::kit::{
    Button, DialogExt as _, DropdownMenu as _, IconName, PopupMenuItem, ScrollableElement as _,
    Switch, h_flex, theme as t,
};
use flikk_core::paths;
use gpui_kit::*;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Place {
    /// Playback copies of media.
    Proxies,
    /// Waveforms, collages, uploads, HTML pages and the like: what the
    /// cache holds that no other row counts. Remade when needed.
    Previews,
    /// Optical flow frames.
    SlowMotion,
    /// Depth maps, subject and person mattes.
    Depth,
    /// Frames the agent looked at, kept for its activity log, and frames
    /// handed to video generation.
    Assistant,
    /// Analysis results: remade only by analyzing again, which can cost.
    Analysis,
    Models,
    Generated,
    /// Audio of the library's sounds, also of ones removed from it, which
    /// projects may still use.
    Sounds,
    Backups,
    /// Frames the background render composed: remade while idle.
    Render,
}

/// Folders under the cache the rows other than Previews own.
const OWNED: [&str; 8] = [
    "proxies",
    "interpolated",
    "depth",
    "mattes",
    "agent",
    "frames",
    "analysis",
    "frame-cache",
];

impl Place {
    const ALL: [Place; 11] = [
        Place::Proxies,
        Place::Previews,
        Place::SlowMotion,
        Place::Depth,
        Place::Assistant,
        Place::Analysis,
        Place::Models,
        Place::Generated,
        Place::Sounds,
        Place::Backups,
        Place::Render,
    ];

    fn label(self) -> &'static str {
        match self {
            Place::Proxies => "Proxies",
            Place::Previews => "Previews",
            Place::SlowMotion => "Slow motion",
            Place::Depth => "Depth & masks",
            Place::Assistant => "Assistant",
            Place::Analysis => "Analysis",
            Place::Models => "Models",
            Place::Generated => "Generated audio",
            Place::Sounds => "Library sounds",
            Place::Backups => "Backups",
            Place::Render => "Render cache",
        }
    }

    /// Where it is, to show in the file manager.
    fn dir(self) -> PathBuf {
        match self {
            Place::Proxies => paths::cache().join("proxies"),
            Place::Previews => paths::cache(),
            Place::SlowMotion => paths::cache().join("interpolated"),
            Place::Depth => paths::cache().join("depth"),
            Place::Assistant => paths::cache().join("agent"),
            Place::Analysis => paths::cache().join("analysis"),
            Place::Models => paths::models(),
            Place::Generated => paths::generated(),
            Place::Sounds => paths::assets(),
            Place::Backups => paths::backups(),
            Place::Render => render_cache::dir(),
        }
    }

    /// The folders it counts and clears.
    fn dirs(self) -> Vec<PathBuf> {
        let cache = paths::cache();
        match self {
            Place::Depth => vec![cache.join("depth"), cache.join("mattes")],
            Place::Assistant => vec![cache.join("agent"), cache.join("frames")],
            // Renders in progress keep their scratch folders; leftovers
            // go at startup (`cache_sweep`).
            Place::Previews => std::fs::read_dir(&cache)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| !OWNED.contains(&n) && n != "render")
                })
                .collect(),
            _ => vec![self.dir()],
        }
    }

    fn size(self) -> u64 {
        match self {
            Place::Render => render_cache::store().bytes(),
            _ => self
                .dirs()
                .iter()
                .map(|d| {
                    if d.file_name().is_some_and(|n| n == "html") {
                        html_size(d)
                    } else if d.is_dir() {
                        dir_size(d)
                    } else {
                        d.metadata().map_or(0, |m| m.len())
                    }
                })
                .sum(),
        }
    }

    fn clearable(self) -> bool {
        !matches!(
            self,
            Place::Models | Place::Generated | Place::Sounds | Place::Backups
        )
    }

    /// Delete what it holds that nothing is using: the browser's profiles
    /// and renders in progress stay.
    fn clear(self) {
        match self {
            Place::Render => render_cache::store().clear(),
            Place::SlowMotion => flikk_analysis::clear_interpolated(),
            _ => {
                for path in self.dirs() {
                    if path.file_name().is_some_and(|n| n == "html") {
                        clear_html(&path);
                    } else if path.is_dir() {
                        let _ = std::fs::remove_dir_all(path);
                    } else {
                        let _ = std::fs::remove_file(path);
                    }
                }
            }
        }
    }
}

/// The HTML pages, without the browser's profiles, which Clear leaves.
fn html_size(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.metadata().ok().filter(|m| m.is_file()))
        .map(|m| m.len())
        .sum()
}

/// HTML pages go; the browser's profiles stay, as one may be in use.
fn clear_html(dir: &Path) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_file() {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map_or(0, |m| m.len()),
            _ => 0,
        })
        .sum()
}

/// Size limits offered for the render cache, in GB.
const RENDER_LIMITS: [u32; 5] = [2, 5, 10, 20, 50];

pub struct StorageTab {
    /// By place, as last measured.
    sizes: Vec<Option<u64>>,
    render: flikk_store::RenderCacheSettings,
    _measure: Option<Task<()>>,
}

impl StorageTab {
    pub fn new(_: &mut Context<Self>) -> Self {
        Self {
            sizes: vec![None; Place::ALL.len()],
            render: render_cache::settings(),
            _measure: None,
        }
    }

    fn set_render(&mut self, settings: flikk_store::RenderCacheSettings, cx: &mut Context<Self>) {
        self.render = settings.clone();
        render_cache::set_settings(settings);
        self.measure(cx);
    }

    /// Background rendering on or off, and the size limit.
    fn render_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let settings = self.render.clone();
        let this = cx.entity();
        h_flex()
            .gap_2()
            .child(
                Button::new("render-limit")
                    .ghost()
                    .xsmall()
                    .label(format!("{} GB", settings.limit_gb))
                    .trailing_icon(IconName::ChevronDown)
                    .tooltip("Size limit")
                    .dropdown_menu({
                        let settings = settings.clone();
                        move |menu, _, _| {
                            RENDER_LIMITS.iter().fold(menu, |menu, &gb| {
                                let this = this.clone();
                                let settings = settings.clone();
                                menu.item(
                                    PopupMenuItem::new(format!("{gb} GB"))
                                        .checked(settings.limit_gb == gb)
                                        .on_click(move |_, _, cx| {
                                            let mut settings = settings.clone();
                                            settings.limit_gb = gb;
                                            this.update(cx, |t, cx| t.set_render(settings, cx));
                                        }),
                                )
                            })
                        }
                    }),
            )
            .child(
                Switch::new("render-background")
                    .checked(settings.background)
                    .tooltip("Render in the background")
                    .on_click(cx.listener(move |this, on: &bool, _, cx| {
                        let mut settings = settings.clone();
                        settings.background = *on;
                        this.set_render(settings, cx);
                    })),
            )
            .into_any_element()
    }

    /// Measure each place off the UI thread.
    pub fn measure(&mut self, cx: &mut Context<Self>) {
        let sizes = cx
            .background_executor()
            .spawn(async { Place::ALL.map(Place::size) });
        self._measure = Some(cx.spawn(async move |this, cx| {
            let sizes = sizes.await;
            let _ = this.update(cx, |this, cx| {
                this.sizes = sizes.into_iter().map(Some).collect();
                cx.notify();
            });
        }));
    }

    /// Everything the user-data folder holds, once measured.
    pub fn total(&self) -> Option<u64> {
        self.sizes.iter().copied().sum()
    }

    fn clear(&mut self, place: Place, window: &mut Window, cx: &mut Context<Self>) {
        if place != Place::Analysis {
            place.clear();
            self.measure(cx);
            return;
        }
        let this = cx.entity().downgrade();
        window.open_confirm(cx, move |dialog, _, _| {
            let this = this.clone();
            dialog
                .title("Clear analysis results?")
                .description("Analyzed media will need to be analyzed again.")
                .destructive()
                .confirm("Clear", move |_, cx| {
                    place.clear();
                    let _ = this.update(cx, |this, cx| this.measure(cx));
                    true
                })
        });
    }
}

impl Render for StorageTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut render = Some(self.render_controls(cx));
        let rows: Vec<_> = Place::ALL
            .iter()
            .enumerate()
            .map(|(ix, &place)| {
                let size = match self.sizes[ix] {
                    Some(0) => "0 KB".to_owned(),
                    Some(bytes) => size_label(bytes),
                    None => String::new(),
                };
                let render = (place == Place::Render).then(|| render.take()).flatten();
                row(place.label(), 130.)
                    .child(meta(size).flex_1().text_size(px(11.)))
                    .children(render)
                    .child(
                        Button::new(("storage-open", ix))
                            .ghost()
                            .small()
                            .icon(IconName::FolderOpen)
                            .tooltip("Show in folder")
                            .on_click(move |_, _, cx| cx.reveal_path(&place.dir())),
                    )
                    // A slot for Clear on every row, so the folder buttons
                    // line up.
                    .child(h_flex().w(px(52.)).flex_none().justify_end().children(
                        place.clearable().then(|| {
                            Button::new(("storage-clear", ix))
                                .small()
                                .label("Clear")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.clear(place, window, cx)
                                }))
                        }),
                    ))
                    .into_any_element()
            })
            .collect();
        page(
            "Storage",
            Some(meta(home_label()).text_size(px(11.)).into_any_element()),
        )
        .child(
            div()
                .flex_none()
                .border_t_1()
                .border_color(t::LN)
                .children(rows),
        )
        .overflow_y_scrollbar()
    }
}

/// The user-data folder, with the home folder as `~`.
fn home_label() -> String {
    let dir = paths::home();
    std::env::var_os("HOME")
        .and_then(|home| {
            dir.strip_prefix(home)
                .ok()
                .map(|rest| Path::new("~").join(rest))
        })
        .unwrap_or(dir)
        .display()
        .to_string()
}
