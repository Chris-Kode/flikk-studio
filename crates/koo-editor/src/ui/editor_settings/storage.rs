//! Settings → Storage: what the user-data folder holds, by kind, each with
//! its folder to open, and a way to clear what can be made again. Models
//! are deleted one by one on the Models page.

use super::{row, size_label};
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use koo_core::paths;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Place {
    /// Proxies, waveforms, collages and the like: remade when needed.
    Previews,
    /// Analysis results: remade only by analyzing again, which can cost.
    Analysis,
    Models,
    Generated,
    Backups,
}

impl Place {
    const ALL: [Place; 5] = [
        Place::Previews,
        Place::Analysis,
        Place::Models,
        Place::Generated,
        Place::Backups,
    ];

    fn label(self) -> &'static str {
        match self {
            Place::Previews => "Previews",
            Place::Analysis => "Analysis",
            Place::Models => "Models",
            Place::Generated => "Generated audio",
            Place::Backups => "Backups",
        }
    }

    fn dir(self) -> PathBuf {
        match self {
            Place::Previews => paths::cache(),
            Place::Analysis => paths::cache().join("analysis"),
            Place::Models => paths::models(),
            Place::Generated => paths::generated(),
            Place::Backups => paths::backups(),
        }
    }

    fn size(self) -> u64 {
        match self {
            Place::Previews => {
                dir_size(&self.dir()).saturating_sub(dir_size(&Place::Analysis.dir()))
            }
            _ => dir_size(&self.dir()),
        }
    }

    fn clearable(self) -> bool {
        matches!(self, Place::Previews | Place::Analysis)
    }

    /// Delete what it holds that nothing is using: the browser's profiles
    /// and renders in progress stay.
    fn clear(self) {
        let dirs: Vec<PathBuf> = match self {
            Place::Previews => ["proxies", "waveforms", "collages", "upload"]
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

pub struct StorageTab {
    /// By place, as last measured.
    sizes: Vec<Option<u64>>,
    _measure: Option<Task<()>>,
}

impl StorageTab {
    pub fn new(_: &mut Context<Self>) -> Self {
        Self {
            sizes: vec![None; Place::ALL.len()],
            _measure: None,
        }
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

    fn clear(&mut self, place: Place, window: &mut Window, cx: &mut Context<Self>) {
        if place == Place::Previews {
            place.clear();
            self.measure(cx);
            return;
        }
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            alert
                .title("Clear analysis results?")
                .description("Analyzed media will need to be analyzed again.")
                .ok_text("Clear")
                .ok_variant(ButtonVariant::Danger)
                .show_cancel(true)
                .on_ok(move |_, _, cx| {
                    place.clear();
                    let _ = this.update(cx, |this, cx| this.measure(cx));
                    true
                })
        });
    }
}

impl Render for StorageTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let rows: Vec<_> = Place::ALL
            .iter()
            .enumerate()
            .map(|(ix, &place)| {
                let size = match self.sizes[ix] {
                    Some(0) => "0 KB".to_owned(),
                    Some(bytes) => size_label(bytes),
                    None => String::new(),
                };
                row(place.label(), cx)
                    .child(div().flex_1().text_xs().text_color(muted).child(size))
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new(("storage-open", ix))
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::FolderOpen)
                                    .tooltip("Show in folder")
                                    .on_click(move |_, _, cx| cx.reveal_path(&place.dir())),
                            )
                            .when(place.clearable(), |el| {
                                el.child(
                                    Button::new(("storage-clear", ix))
                                        .outline()
                                        .xsmall()
                                        .label("Clear")
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.clear(place, window, cx)
                                        })),
                                )
                            }),
                    )
                    .into_any_element()
            })
            .collect();
        v_flex().size_full().children(rows)
    }
}
