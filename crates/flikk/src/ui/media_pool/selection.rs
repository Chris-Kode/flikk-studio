//! What the pool has selected, and the rubber band that selects across
//! items.

use super::{Entry, MediaPoolPanel};
use crate::ui::kit::theme as t;
use flikk_core::{BinId, MediaId};
use gpui_kit::*;

/// What the pool has selected. Its parts change only through these
/// methods, so they always change together.
#[derive(Default)]
pub(super) struct Selection {
    /// The item that leads: the details show it.
    lead: Option<MediaId>,
    /// Items selected with it, by cmd/ctrl- or shift-click or the band.
    also: Vec<MediaId>,
    /// Selected folders.
    folders: Vec<BinId>,
    /// What was clicked or stepped to last: where shift-click runs
    /// from and the arrow keys move from.
    cursor: Option<Entry>,
}

impl Selection {
    pub(super) fn lead(&self) -> Option<MediaId> {
        self.lead
    }

    pub(super) fn cursor(&self) -> Option<Entry> {
        self.cursor
    }

    pub(super) fn folders(&self) -> &[BinId] {
        &self.folders
    }

    /// The selected items, the lead first.
    pub(super) fn items(&self) -> Vec<MediaId> {
        self.lead
            .into_iter()
            .chain(self.also.iter().copied())
            .collect()
    }

    /// Everything selected: folders, then items with the lead first.
    pub(super) fn entries(&self) -> Vec<Entry> {
        let folders = self.folders.iter().map(|f| Entry::Folder(*f));
        let items = self.items().into_iter().map(Entry::Media);
        folders.chain(items).collect()
    }

    /// How many items and folders are selected.
    pub(super) fn count(&self) -> usize {
        self.folders.len() + usize::from(self.lead.is_some()) + self.also.len()
    }

    pub(super) fn has_item(&self, id: MediaId) -> bool {
        self.lead == Some(id) || self.also.contains(&id)
    }

    pub(super) fn has_folder(&self, id: BinId) -> bool {
        self.folders.contains(&id)
    }

    /// Just `entry`, with the cursor on it.
    pub(super) fn select_one(&mut self, entry: Entry) {
        self.set(&[entry]);
        self.cursor = Some(entry);
    }

    /// Select `entries`; the lead stays so if it is among them, else
    /// the first item leads. The cursor stays.
    pub(super) fn set(&mut self, entries: &[Entry]) {
        let media: Vec<MediaId> = entries
            .iter()
            .filter_map(|e| match e {
                Entry::Media(m) => Some(*m),
                Entry::Folder(_) => None,
            })
            .collect();
        self.folders = entries
            .iter()
            .filter_map(|e| match e {
                Entry::Folder(f) => Some(*f),
                Entry::Media(_) => None,
            })
            .collect();
        let lead = self
            .lead
            .filter(|s| media.contains(s))
            .or(media.first().copied());
        self.lead = lead;
        self.also = media.into_iter().filter(|m| Some(*m) != lead).collect();
    }

    /// Cmd/ctrl-click: add `entry`, or drop it (the next item leads
    /// if it led). The cursor moves to it.
    pub(super) fn toggle(&mut self, entry: Entry) {
        let mut entries = self.entries();
        match entries.iter().position(|e| *e == entry) {
            Some(i) => {
                entries.remove(i);
            }
            None => entries.push(entry),
        }
        self.set(&entries);
        self.cursor = Some(entry);
    }

    /// Forget `media` and the folders `gone` names, once they are
    /// removed. The rest stays selected; the next item leads if the
    /// lead went.
    pub(super) fn remove(&mut self, media: &[MediaId], gone: impl Fn(BinId) -> bool) {
        let out = |e: &Entry| match *e {
            Entry::Media(m) => media.contains(&m),
            Entry::Folder(f) => gone(f),
        };
        let left: Vec<Entry> = self.entries().into_iter().filter(|e| !out(e)).collect();
        self.set(&left);
        if self.cursor.as_ref().is_some_and(out) {
            self.cursor = None;
        }
    }

    /// Select nothing and drop the cursor; whether anything was
    /// selected.
    pub(super) fn reset(&mut self) -> bool {
        let had = self.count() > 0;
        *self = Self::default();
        had
    }
}

/// A rubber band across the items, in their scrolling space.
pub(super) struct Band {
    from: Point<Pixels>,
    to: Point<Pixels>,
    /// The selection it adds to.
    base: Vec<Entry>,
    /// Past a few pixels: a press that stays put only deselects.
    moved: bool,
}

impl Band {
    fn rect(&self) -> Bounds<Pixels> {
        let (a, b) = (self.from, self.to);
        Bounds::from_corners(
            point(a.x.min(b.x), a.y.min(b.y)),
            point(a.x.max(b.x), a.y.max(b.y)),
        )
    }
}

impl MediaPoolPanel {
    /// A press on empty space starts a rubber band; with shift or cmd/ctrl
    /// it adds to the selection, else it starts over.
    pub(super) fn band_start(
        &mut self,
        ev: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .placed
            .borrow()
            .iter()
            .any(|(_, b)| b.contains(&ev.position))
        {
            return;
        }
        window.focus(&self.focus, cx);
        let additive = ev.modifiers.shift || ev.modifiers.secondary();
        if !additive {
            self.deselect(cx);
        }
        let at = self.in_items(ev.position);
        self.band = Some(Band {
            from: at,
            to: at,
            base: self.sel.entries(),
            moved: false,
        });
    }

    /// The band follows the pointer, selecting what it covers.
    pub(super) fn band_move(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        if self.band.is_none() {
            return;
        }
        if ev.pressed_button != Some(MouseButton::Left) {
            self.band_end(cx);
            return;
        }
        let at = self.in_items(ev.position);
        let offset = self.scroll.offset();
        let Some(band) = self.band.as_mut() else {
            return;
        };
        band.to = at;
        let d = band.to - band.from;
        band.moved |= d.x.abs() > px(3.) || d.y.abs() > px(3.);
        if !band.moved {
            return;
        }
        let rect = band.rect();
        let hits: Vec<Entry> = self
            .placed
            .borrow()
            .iter()
            .filter(|(_, b)| Bounds::new(b.origin - offset, b.size).intersects(&rect))
            .map(|(e, _)| *e)
            .collect();
        let base = band.base.clone();
        let picked: Vec<Entry> = self
            .order(cx)
            .into_iter()
            .filter(|e| base.contains(e) || hits.contains(e))
            .collect();
        self.sel.set(&picked);
        cx.notify();
    }

    pub(super) fn band_end(&mut self, cx: &mut Context<Self>) {
        if self.band.take().is_some() {
            cx.notify();
        }
    }

    /// The band over the items area, in its coordinates.
    pub(super) fn band_overlay(&self) -> Option<Div> {
        let band = self.band.as_ref().filter(|b| b.moved)?;
        let rect = band.rect();
        let origin = rect.origin + self.scroll.offset() - self.area.get().origin;
        Some(
            div()
                .absolute()
                .left(origin.x)
                .top(origin.y)
                .w(rect.size.width)
                .h(rect.size.height)
                .border_1()
                .border_color(t::alpha(t::MK, 0.8))
                .bg(t::alpha(t::MK, 0.12)),
        )
    }
}
