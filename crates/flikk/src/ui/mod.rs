//! Panels. Each file owns one panel's internals; `app.rs` owns the layout.
//! Every panel is built with `new(state, window, cx)`, observes the shared
//! [`EditorState`](crate::state::EditorState) and renders itself.

pub mod blockout;
pub mod caption_style;
pub mod chat;
pub mod code_editor;
pub mod deliver;
pub mod editor_settings;
pub mod format;
pub mod gpu_jobs;
pub mod inspector;
pub mod kit;
pub mod lettering;
pub mod library;
pub mod listen;
pub mod media_pool;
pub mod mixer;
pub mod notices;
pub mod project_settings;
pub mod provider_ask;
pub mod scopes;
pub mod settings;
pub mod timeline;
pub mod transcript;
pub mod transport;
pub mod viewer;
pub mod voices;
pub mod welcome;

use gpui_kit::*;
use kit::theme as t;

/// Repaints the entity every `period` for as long as `busy` says there is
/// progress to show, checking after each repaint; the first repaint is a
/// period from now. Once `busy` is false or the entity is gone it stops,
/// and `stopped` runs on the entity (to forget the task). The caller keeps
/// the task: dropping it stops the repaints.
pub fn repaint_while<T: 'static>(
    cx: &mut Context<T>,
    period: std::time::Duration,
    busy: impl Fn(&T, &App) -> bool + 'static,
    stopped: impl FnOnce(&mut T) + 'static,
) -> Task<()> {
    cx.spawn(async move |this, cx| {
        loop {
            cx.background_executor().timer(period).await;
            let busy = this.update(cx, |this, cx| {
                cx.notify();
                busy(this, cx)
            });
            if !matches!(busy, Ok(true)) {
                break;
            }
        }
        let _ = this.update(cx, |this, _| stopped(this));
    })
}

/// Drag payload for pool media dropped on the timeline or viewer: the
/// item grabbed and, when it was part of a selection, the rest of it.
#[derive(Clone, Debug)]
pub struct MediaDrag {
    pub media: flikk_core::MediaId,
    pub name: SharedString,
    pub more: Vec<flikk_core::MediaId>,
    /// Folders selected with it, which a folder takes; the timeline
    /// doesn't.
    pub folders: Vec<flikk_core::BinId>,
}

impl MediaDrag {
    pub fn one(media: flikk_core::MediaId, name: SharedString) -> Self {
        Self {
            media,
            name,
            more: Vec::new(),
            folders: Vec::new(),
        }
    }

    /// Every item dragged, the grabbed one first.
    pub fn all(&self) -> Vec<flikk_core::MediaId> {
        std::iter::once(self.media)
            .chain(self.more.iter().copied())
            .collect()
    }
}

impl Render for MediaDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        match self.more.len() + self.folders.len() {
            0 => drag_chip(self.name.clone()),
            n => drag_chip(format!("{} +{n}", self.name).into()),
        }
    }
}

/// Drag payload for pool folders, with the items selected with them,
/// dropped on another folder.
#[derive(Clone, Debug)]
pub struct FolderDrag {
    pub folders: Vec<flikk_core::BinId>,
    pub media: Vec<flikk_core::MediaId>,
    pub name: SharedString,
}

impl Render for FolderDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        match self.folders.len() + self.media.len() - 1 {
            0 => drag_chip(self.name.clone()),
            n => drag_chip(format!("{} +{n}", self.name).into()),
        }
    }
}

/// Drag payload for a sound from the library, added to the pool when
/// dropped on the timeline or the chat.
#[derive(Clone, Debug)]
pub struct SoundDrag {
    pub sound: flikk_core::LibraryId,
    pub name: SharedString,
}

impl Render for SoundDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        drag_chip(self.name.clone())
    }
}

/// Drag payload for an FX library item: an effect dropped on a clip, a
/// transition on a cut or clip edge, a generator on a track. The chat takes
/// any of them as a reference.
#[derive(Clone, Debug)]
pub struct FxDrag {
    pub fx: flikk_core::fx::FxRef,
    pub kind: flikk_core::fx::FxKind,
    pub name: SharedString,
    /// A library program, copied into the project when dropped (`fx` is
    /// then a placeholder).
    pub library: Option<flikk_core::LibraryId>,
}

impl FxDrag {
    /// The program to use where it is dropped, copying a library program
    /// into the project first.
    pub fn program(
        &self,
        state: &Entity<crate::state::EditorState>,
        cx: &mut App,
    ) -> Option<flikk_core::fx::FxRef> {
        match self.library {
            Some(id) => state.update(cx, |s, cx| library::shared::use_program(s, id, cx)),
            None => Some(self.fx.clone()),
        }
    }
}

impl Render for FxDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        drag_chip(self.name.clone())
    }
}

/// Drag payload for an audio effect from the library, dropped on an audio
/// clip or a track header.
#[derive(Clone, Debug)]
pub struct AudioFxDrag {
    pub kind: flikk_core::audiofx::AudioFxKind,
    pub name: SharedString,
}

impl Render for AudioFxDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        drag_chip(self.name.clone())
    }
}

/// Drag payload for project objects (clips, markers, tracks, transcript
/// ranges…) dropped on the chat composer, which inserts them as `@`
/// references.
#[derive(Clone, Debug)]
pub struct RefDrag {
    /// Reference token text as the agent reads it (e.g. `@clip 3f2a91c0`)
    /// and the label its chip shows.
    pub refs: Vec<(String, SharedString)>,
}

impl Render for RefDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let label = match self.refs.as_slice() {
            [(_, one)] => one.clone(),
            many => format!("{} items", many.len()).into(),
        };
        drag_chip(label)
    }
}

/// The chip that follows the pointer while something is dragged.
fn drag_chip(label: SharedString) -> Div {
    div()
        .flex()
        .items_center()
        .h(px(26.))
        .px(px(8.))
        .rounded(px(6.))
        .border_1()
        .border_color(t::TIP_LN)
        .bg(t::TIP)
        .shadow(t::floating_shadow())
        .text_size(px(12.))
        .text_color(t::T1)
        .whitespace_nowrap()
        .child(label)
}
