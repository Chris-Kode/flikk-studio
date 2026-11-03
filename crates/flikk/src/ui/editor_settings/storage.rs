//! Settings → Storage: what the user-data folder holds, by kind, each with
//! its folder to open, and a way to clear what can be made again. Models
//! are deleted one by one on the Models page. The render cache's row also
//! turns background rendering on and off and sets its size limit.

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
    /// Proxies, waveforms, collages, subject mattes and the like: remade
    /// when needed.
    Previews,
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

impl Place {
    const ALL: [Place; 7] = [
        Place::Previews,
        Place::Analysis,
        Place::Models,
        Place::Generated,
        Place::Sounds,
        Place::Backups,
        Place::Render,
    ];

    fn label(self) -> &'static str {
        match self {
            Place::Previews => "Previews",
            Place::Analysis => "Analysis",
            Place::Models => "Models",
            Place::Generated => "Generated audio",
            Place::Sounds => "Library sounds",
            Place::Backups => "Backups",
            Place::Render => "Render cache",
        }
    }

    fn dir(self) -> PathBuf {
        match self {
            Place::Previews => paths::cache(),
            Place::Analysis => paths::cache().join("analysis"),
            Place::Models => paths::models(),
            Place::Generated => paths::generated(),
            Place::Sounds => paths::assets(),
            Place::Backups => paths::backups(),
            Place::Render => render_cache::dir(),
        }
    }

    fn size(self) -> u64 {
        match self {
            Place::Previews => dir_size(&self.dir())
                .saturating_sub(dir_size(&Place::Analysis.dir()))
                .saturating_sub(dir_size(&Place::Render.dir())),
            Place::Render => render_cache::store().bytes(),
            _ => dir_size(&self.dir()),
        }
    }

    fn clearable(self) -> bool {
        matches!(self, Place::Previews | Place::Analysis | Place::Render)
    }

    /// Delete what it holds that nothing is using: the browser's profiles
    /// and renders in progress stay.
    fn clear(self) {
        if self == Place::Render {
            render_cache::store().clear();
            return;
        }
        let dirs: Vec<PathBuf> = match self {
            Place::Previews => [
                "proxies",
                "waveforms",
                "collages",
                "upload",
                "mattes",
                "hdr",
                "interpolated",
                "depth",
                "coreml",
                "respeak",
            ]
            .iter()
            .map(|d| paths::cache().join(d))
            .collect(),
            Place::Analysis => vec![self.dir()],
            _ => Vec::new(),
        };
        for dir in dirs {
            let _ = std::fs::remove_dir_all(dir);
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
        if matches!(place, Place::Previews | Place::Render) {
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
