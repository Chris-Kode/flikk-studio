//! Settings → Storage: the user-data folders a user has a stake in, each
//! with its folder to open: the cache as one (everything the editor makes
//! again by itself), with Clear, then models, generated audio, library
//! sounds and backups. Models are deleted one by one on the Models page.
//! Analysis results are left out: they are not cleared with the cache, as
//! making them again can cost. Background rendering is turned on and off,
//! and its size limit set, on a row of its own.

use super::{meta, page, row, size_label};
use crate::render_cache;
use crate::ui::kit::{
    Button, DropdownMenu as _, IconName, PopupMenuItem, ScrollableElement as _, Switch, h_flex,
    theme as t,
};
use flikk_core::paths;
use gpui_kit::*;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Place {
    /// Proxies, previews, optical flow frames, depth maps and mattes,
    /// rendered frames, the agent's images, scratch: made again when
    /// needed.
    Cache,
    Models,
    Generated,
    /// Audio of the library's sounds, also of ones removed from it, which
    /// projects may still use.
    Sounds,
    Backups,
}

/// Under the cache, what Clear leaves: analysis results, which can cost
/// to make again, and renders' scratch folders, which one in progress
/// uses (leftovers go at startup, `cache_sweep`).
const KEPT: [&str; 2] = ["analysis", "render"];

const LABEL_W: f32 = 150.;

impl Place {
    const ALL: [Place; 5] = [
        Place::Cache,
        Place::Models,
        Place::Generated,
        Place::Sounds,
        Place::Backups,
    ];

    fn label(self) -> &'static str {
        match self {
            Place::Cache => "Cache",
            Place::Models => "Models",
            Place::Generated => "Generated audio",
            Place::Sounds => "Library sounds",
            Place::Backups => "Backups",
        }
    }

    /// Where it is, to show in the file manager.
    fn dir(self) -> PathBuf {
        match self {
            Place::Cache => paths::cache(),
            Place::Models => paths::models(),
            Place::Generated => paths::generated(),
            Place::Sounds => paths::assets(),
            Place::Backups => paths::backups(),
        }
    }

    fn size(self) -> u64 {
        match self {
            Place::Cache => cleared().iter().map(|p| path_size(p)).sum(),
            _ => dir_size(&self.dir()),
        }
    }

    fn clearable(self) -> bool {
        self == Place::Cache
    }

    /// Delete what it holds that nothing is using: the browser's profiles
    /// and renders in progress stay.
    fn clear(self) {
        if self != Place::Cache {
            return;
        }
        // Their indexes forget what they held.
        render_cache::store().clear();
        flikk_analysis::clear_interpolated();
        for path in cleared() {
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

/// What the cache's Clear removes, and its size counts.
fn cleared() -> Vec<PathBuf> {
    std::fs::read_dir(paths::cache())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| !KEPT.contains(&n))
        })
        .collect()
}

fn path_size(path: &Path) -> u64 {
    if path.file_name().is_some_and(|n| n == "html") {
        html_size(path)
    } else if path.is_dir() {
        dir_size(path)
    } else {
        path.metadata().map_or(0, |m| m.len())
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

/// Size limits offered for rendered frames, in GB.
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

    fn clear(&mut self, place: Place, cx: &mut Context<Self>) {
        place.clear();
        self.measure(cx);
    }
}

impl Render for StorageTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut rows: Vec<_> = Place::ALL
            .iter()
            .enumerate()
            .map(|(ix, &place)| {
                let size = match self.sizes[ix] {
                    Some(0) => "0 KB".to_owned(),
                    Some(bytes) => size_label(bytes),
                    None => String::new(),
                };
                row(place.label(), LABEL_W)
                    .child(meta(size).flex_1().text_size(px(11.)))
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
                                .on_click(cx.listener(move |this, _, _, cx| this.clear(place, cx)))
                        }),
                    ))
                    .into_any_element()
            })
            .collect();
        rows.push(
            row("Background rendering", LABEL_W)
                .child(div().flex_1())
                .child(self.render_controls(cx))
                .into_any_element(),
        );
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
