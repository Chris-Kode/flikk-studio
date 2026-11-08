//! Folders: the tree, creating, renaming, deleting and moving into them,
//! the path bar and the folder tiles.

use super::look::{inset_ring, ring};
use super::menus::{bin_menu, folder_more};
use super::{Entry, Layout, MediaPoolPanel, column};
use crate::ui::kit::theme as t;
use crate::ui::kit::{ContextMenuExt as _, DialogExt as _, Icon, IconName, Input, h_flex};
use crate::ui::{FolderDrag, MediaDrag};
use flikk_core::command::Command;
use flikk_core::model::{Bin, MediaItem};
use flikk_core::{BinId, MediaId};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::HashMap;

/// The bins by parent, for walking up from a bin.
pub(super) struct BinTree(HashMap<BinId, Option<BinId>>);

impl BinTree {
    pub(super) fn of(bins: &[Bin]) -> Self {
        Self(bins.iter().map(|b| (b.id, b.parent)).collect())
    }

    /// Where something in `bin` shows: that bin, or the top level when
    /// the bin is gone.
    pub(super) fn home(&self, bin: Option<BinId>) -> Option<BinId> {
        bin.filter(|b| self.0.contains_key(b))
    }

    /// `bin`, then each bin it sits in, up to the top level.
    pub(super) fn ancestry(&self, bin: Option<BinId>) -> impl Iterator<Item = BinId> + '_ {
        let mut at = self.home(bin);
        // A bad parent loop stops after this many.
        let mut left = self.0.len();
        std::iter::from_fn(move || {
            let here = at.filter(|_| left > 0)?;
            left -= 1;
            at = self.home(self.0.get(&here).copied().flatten());
            Some(here)
        })
    }

    /// Whether something in `bin` is in `open` or a bin inside it.
    pub(super) fn within(&self, bin: Option<BinId>, open: BinId) -> bool {
        self.ancestry(bin).any(|b| b == open)
    }
}

/// `bins` in tree order, each with how deep it sits.
pub(super) fn bin_tree(bins: &[Bin]) -> Vec<(&Bin, usize)> {
    fn under<'a>(
        bins: &'a [Bin],
        parent: Option<BinId>,
        depth: usize,
        out: &mut Vec<(&'a Bin, usize)>,
    ) {
        for bin in bins.iter().filter(|b| b.parent == parent) {
            out.push((bin, depth));
            under(bins, Some(bin.id), depth + 1, out);
        }
    }
    let mut out = Vec::new();
    under(bins, None, 0, &mut out);
    // A bin whose parent is gone sits at the top.
    for bin in bins {
        if !out.iter().any(|(b, _)| b.id == bin.id) {
            out.push((bin, 0));
            under(bins, Some(bin.id), 1, &mut out);
        }
    }
    out
}

impl MediaPoolPanel {
    /// Put `media` and `folders` in folder `to` (`None`: the top level),
    /// as one undo step. A folder doesn't go into itself or a folder in
    /// it.
    pub(super) fn move_entries(
        &mut self,
        media: &[MediaId],
        folders: &[BinId],
        to: Option<BinId>,
        cx: &mut Context<Self>,
    ) {
        let project = self.state.read(cx).bus.project();
        let tree = BinTree::of(&project.bins);
        let moving: Vec<BinId> = folders
            .iter()
            .copied()
            .filter(|f| !tree.within(to, *f))
            .collect();
        let mut commands = Vec::new();
        if project
            .bins
            .iter()
            .any(|b| moving.contains(&b.id) && b.parent != to)
        {
            let bins = project
                .bins
                .iter()
                .map(|b| Bin {
                    parent: if moving.contains(&b.id) { to } else { b.parent },
                    ..b.clone()
                })
                .collect();
            commands.push(Command::SetBins { bins });
        }
        commands.extend(
            media
                .iter()
                .filter_map(|id| project.media(*id))
                .filter(|item| item.bin != to)
                .map(|item| Command::UpdateMedia {
                    media: MediaItem {
                        bin: to,
                        ..item.clone()
                    },
                }),
        );
        if !commands.is_empty() {
            self.state
                .update(cx, |s, cx| s.execute_batch(commands, "move_to_bin", cx));
        }
    }

    fn bins(&self, cx: &App) -> Vec<Bin> {
        self.state.read(cx).bus.project().bins.clone()
    }

    /// A folder in `parent`, which opens, named as it is typed.
    pub(super) fn new_bin(
        &mut self,
        parent: Option<BinId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_bin(parent, cx);
        // Searching hides folders; the new one has to show to be named.
        self.search.update(cx, |s, cx| s.set_value("", window, cx));
        let mut bins = self.bins(cx);
        let n = bins.len() + 1;
        let bin = Bin {
            id: BinId::new(),
            name: format!("Folder {n}"),
            parent,
        };
        let id = bin.id;
        bins.push(bin);
        self.state.update(cx, |s, cx| {
            s.execute(Command::SetBins { bins }, cx);
        });
        self.start_rename(id, window, cx);
    }

    pub(super) fn start_rename(&mut self, id: BinId, window: &mut Window, cx: &mut Context<Self>) {
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

    /// Remove `media` and delete `folders` with everything in them; one
    /// undo step. Items alone ask as [`Self::confirm_remove`] does; empty
    /// folders go at once; otherwise this asks first, saying what goes.
    pub(super) fn confirm_delete(
        &mut self,
        media: Vec<MediaId>,
        folders: Vec<BinId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if folders.is_empty() {
            self.confirm_remove(media, window, cx);
            return;
        }
        let state = self.state.read(cx);
        let project = state.bus.project();
        let tree = BinTree::of(&project.bins);
        let gone = |bin: Option<BinId>| folders.iter().any(|f| tree.within(bin, *f));
        let inner = project
            .bins
            .iter()
            .filter(|b| !folders.contains(&b.id) && gone(Some(b.id)))
            .count();
        let held: Vec<MediaId> = project
            .media
            .iter()
            .filter(|m| gone(m.bin))
            .map(|m| m.id)
            .collect();
        let mut all = media.clone();
        all.extend(held.iter().filter(|h| !media.contains(h)));
        if all.is_empty() && inner == 0 {
            self.delete_entries(&folders, Vec::new(), cx);
            return;
        }
        let clips: usize = all.iter().map(|m| state.clips_using(*m).len()).sum();
        let plural = |n: usize, one: &str, many: &str| match n {
            1 => format!("1 {one}"),
            n => format!("{n} {many}"),
        };
        let clips_go = (clips > 0).then(|| {
            format!(
                " {} on the timeline go too.",
                plural(clips, "clip", "clips")
            )
        });
        let (title, description, confirm) = match (folders.as_slice(), media.is_empty()) {
            ([one], true) => {
                let name = project
                    .bins
                    .iter()
                    .find(|b| b.id == *one)
                    .map_or(String::new(), |b| b.name.clone());
                let held: Vec<String> =
                    [(held.len(), "item", "items"), (inner, "folder", "folders")]
                        .into_iter()
                        .filter(|(n, _, _)| *n > 0)
                        .map(|(n, one, many)| plural(n, one, many))
                        .collect();
                (
                    format!("Delete “{name}” and everything in it?"),
                    format!("It holds {}.", held.join(" and ")),
                    "Delete",
                )
            }
            _ => (
                format!(
                    "Remove {} items from the project?",
                    folders.len() + media.len()
                ),
                "Folders go with everything in them.".to_owned(),
                "Remove",
            ),
        };
        let description = description + clips_go.as_deref().unwrap_or("");
        let panel = cx.entity().downgrade();
        window.open_confirm(cx, move |dialog, _, _| {
            let (panel, folders, all) = (panel.clone(), folders.clone(), all.clone());
            dialog
                .title(title.clone())
                .description(description.clone())
                .destructive()
                .confirm(confirm, move |_, cx| {
                    let _ = panel.update(cx, |this, cx| {
                        this.delete_entries(&folders, all.clone(), cx)
                    });
                    true
                })
        });
    }

    /// Remove `folders`, the folders in them and `media`.
    fn delete_entries(&mut self, folders: &[BinId], media: Vec<MediaId>, cx: &mut Context<Self>) {
        let bins = self.bins(cx);
        let tree = BinTree::of(&bins);
        let gone = |bin: Option<BinId>| folders.iter().any(|f| tree.within(bin, *f));
        let keep: Vec<Bin> = bins.iter().filter(|b| !gone(Some(b.id))).cloned().collect();
        // Out of a folder that goes, to the nearest one that stays.
        if gone(self.bin) {
            let stay = tree.ancestry(self.bin).find(|b| !gone(Some(*b)));
            self.open_bin(stay, cx);
        }
        self.sel.remove(&media, |f| gone(Some(f)));
        self.state
            .update(cx, |s, cx| s.remove_from_pool(&media, Some(keep), cx));
        cx.notify();
    }

    pub(super) fn finish_rename(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(id) = self.renaming.take() else {
            return;
        };
        if !name.is_empty() {
            let mut bins = self.bins(cx);
            if let Some(b) = bins.iter_mut().find(|b| b.id == id) {
                b.name = name;
                self.state.update(cx, |s, cx| {
                    s.execute(Command::SetBins { bins }, cx);
                });
            }
        }
        cx.notify();
    }

    /// The folders in the open folder, each with how many items it holds,
    /// counting its own folders'; none while searching.
    pub(super) fn folders(&self, cx: &App) -> Vec<(Bin, usize)> {
        if self.flattened(cx) {
            return Vec::new();
        }
        let project = self.state.read(cx).bus.project();
        let tree = BinTree::of(&project.bins);
        let mut counts: HashMap<BinId, usize> = HashMap::new();
        for item in &project.media {
            for bin in tree.ancestry(item.bin) {
                *counts.entry(bin).or_default() += 1;
            }
        }
        project
            .bins
            .iter()
            .filter(|b| tree.home(b.parent) == self.bin)
            .map(|b| (b.clone(), counts.get(&b.id).copied().unwrap_or(0)))
            .collect()
    }

    /// Open `bin` in the pool; the selection stays behind.
    pub(super) fn open_bin(&mut self, bin: Option<BinId>, cx: &mut Context<Self>) {
        if self.bin != bin {
            self.bin = bin;
            self.sel.reset();
            self.scroll.set_offset(point(px(0.), px(0.)));
        }
        cx.notify();
    }

    /// Where the pool is: Media, then each folder down to the open one,
    /// the middle ones folded on a deep path. Each place takes dropped
    /// media.
    pub(super) fn path_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let project = self.state.read(cx).bus.project();
        let tree = BinTree::of(&project.bins);
        let mut path: Vec<Bin> = tree
            .ancestry(self.bin)
            .filter_map(|id| project.bins.iter().find(|b| b.id == id).cloned())
            .collect();
        path.reverse();
        let mut crumbs: Vec<(Option<BinId>, SharedString)> = vec![(None, "Media".into())];
        let folded = path.len() > 2;
        let shown = if folded {
            &path[path.len() - 2..]
        } else {
            &path[..]
        };
        if folded {
            // "…" goes to the bin above the ones shown.
            crumbs.push((Some(path[path.len() - 3].id), "…".into()));
        }
        crumbs.extend(shown.iter().map(|b| (Some(b.id), b.name.clone().into())));
        let last = crumbs.len() - 1;
        let mut bar = h_flex()
            .flex_none()
            .h(px(28.))
            .px(px(8.))
            .gap(px(2.))
            .text_size(px(12.));
        for (i, (bin, label)) in crumbs.into_iter().enumerate() {
            if i > 0 {
                bar = bar.child(Icon::new(IconName::ChevronRight).size(px(10.)).color(t::T3));
            }
            let current = i == last;
            bar = bar.child(
                div()
                    .id(("crumb", i))
                    .flex_shrink(1.)
                    .min_w(px(16.))
                    .max_w(px(140.))
                    .h(px(22.))
                    .px(px(5.))
                    .flex()
                    .items_center()
                    .rounded(t::RADIUS_SM)
                    .text_color(if current { t::T1 } else { t::T3 })
                    .when(!current, |d| {
                        d.cursor_pointer().hover(|s| s.bg(t::HV).text_color(t::T1))
                    })
                    .drag_over::<MediaDrag>(|s, _, _, _| s.bg(t::AC_WASH))
                    .drag_over::<FolderDrag>(|s, _, _, _| s.bg(t::AC_WASH))
                    .drag_over::<ExternalPaths>(|s, _, _, _| s.bg(t::AC_WASH))
                    .child(div().truncate().child(label))
                    .on_click(cx.listener(move |this, _, _, cx| this.open_bin(bin, cx)))
                    .on_drop(cx.listener(move |this, drag: &MediaDrag, _, cx| {
                        this.move_entries(&drag.all(), &drag.folders, bin, cx)
                    }))
                    .on_drop(cx.listener(move |this, drag: &FolderDrag, _, cx| {
                        this.move_entries(&drag.media, &drag.folders, bin, cx)
                    }))
                    .on_drop(cx.listener(move |this, paths: &ExternalPaths, _, cx| {
                        this.import(paths.paths().to_vec(), bin, cx)
                    })),
            );
        }
        bar
    }

    /// A folder among the items: a click selects it and a double click
    /// opens it, dropped media and folders go in, and its menu renames,
    /// moves or deletes it. A tile in the grid, a row in the list.
    pub(super) fn folder(
        &self,
        bin: &Bin,
        count: usize,
        wide: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = bin.id;
        let renaming = self.renaming == Some(id);
        let name = if renaming {
            div()
                .flex_1()
                .min_w_0()
                .child(Input::new(&self.bin_input).small())
                .into_any_element()
        } else {
            div()
                .min_w_0()
                .truncate()
                .child(bin.name.clone())
                .into_any_element()
        };
        let holds = match count {
            0 => "Empty".to_owned(),
            1 => "1 item".to_owned(),
            n => format!("{n} items"),
        };
        let panel = cx.entity();
        let entry = Entry::Folder(id);
        let selected = self.sel.has_folder(id);
        // Grabbing a selected folder drags the selection.
        let drag = {
            let (mut media, folders) = if selected {
                self.split_of(entry)
            } else {
                (Vec::new(), vec![id])
            };
            media = self.in_pool_order(&media, cx);
            FolderDrag {
                folders,
                media,
                name: bin.name.clone().into(),
            }
        };
        let el = div()
            .id(ElementId::Name(format!("folder-{id}").into()))
            .group(SharedString::from(format!("folder-{id}")))
            .min_w_0()
            .cursor_pointer()
            .when(!renaming, |d| {
                d.on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    this.click(entry, ev.click_count() >= 2, window, cx)
                }))
                .on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.clone()))
            })
            .on_drop(cx.listener(move |this, drag: &MediaDrag, _, cx| {
                this.move_entries(&drag.all(), &drag.folders, Some(id), cx)
            }))
            .on_drop(cx.listener(move |this, drag: &FolderDrag, _, cx| {
                this.move_entries(&drag.media, &drag.folders, Some(id), cx)
            }))
            .on_drop(cx.listener(move |this, paths: &ExternalPaths, _, cx| {
                this.import(paths.paths().to_vec(), Some(id), cx)
            }));
        let el = el
            .relative()
            .child(self.place_marker(entry))
            .when(self.sel.cursor() == Some(entry), |d| {
                d.child(self.reveal_marker())
            });
        let more = folder_more(id, &panel);
        let el = match self.layout {
            Layout::Grid => el
                .flex()
                .flex_col()
                .text_size(px(11.))
                .child(
                    div()
                        .w_full()
                        .aspect_ratio(16. / 9.)
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(t::RADIUS_SM)
                        .border_1()
                        .border_color(t::LN)
                        .bg(t::R1)
                        .group_hover(format!("folder-{id}"), |s| s.border_color(t::LN_HV))
                        .when(selected, |d| d.shadow(ring()))
                        .drag_over::<MediaDrag>(|s, _, _, _| s.bg(t::AC_WASH).border_color(t::AC))
                        .drag_over::<FolderDrag>(|s, _, _, _| s.bg(t::AC_WASH).border_color(t::AC))
                        .drag_over::<ExternalPaths>(|s, _, _, _| {
                            s.bg(t::AC_WASH).border_color(t::AC)
                        })
                        .relative()
                        .child(Icon::new(IconName::Folder).size(px(22.)).color(t::T2))
                        .child(more.absolute().top(px(3.)).right(px(3.))),
                )
                .child(div().mt(px(4.)).flex().child(name))
                .child(div().text_size(px(10.)).text_color(t::T3).child(holds)),
            Layout::List => el
                .h(px(30.))
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(8.))
                .rounded(t::RADIUS)
                .text_size(px(12.))
                .when(selected, |d| {
                    d.bg(t::alpha(t::AC, 0.1))
                        .shadow(inset_ring(t::alpha(t::AC, 0.4)))
                })
                .when(!selected, |d| d.hover(|s| s.bg(t::HV)))
                .drag_over::<MediaDrag>(|s, _, _, _| s.bg(t::AC_WASH))
                .drag_over::<FolderDrag>(|s, _, _, _| s.bg(t::AC_WASH))
                .drag_over::<ExternalPaths>(|s, _, _, _| s.bg(t::AC_WASH))
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .w(px(36.))
                        .h(px(20.))
                        .rounded(px(3.))
                        .bg(t::R1)
                        .child(Icon::new(IconName::Folder).size(px(12.)).color(t::T2)),
                )
                .child(h_flex().flex_1().min_w_0().child(name))
                .child(column(holds, 56.).text_color(t::T3))
                .when(wide, |d| d.child(column("", 120.)).child(column("", 90.)))
                .child(more),
        };
        el.context_menu(move |menu, window, cx| bin_menu(menu, &panel, id, window, cx))
            .into_any_element()
    }
}
