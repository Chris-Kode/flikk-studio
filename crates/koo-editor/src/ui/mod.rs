//! Panels. Each file owns one panel's internals; `app.rs` owns the layout.
//! Every panel is built with `new(state, window, cx)`, observes the shared
//! [`EditorState`](crate::state::EditorState) and renders itself.

pub mod caption_style;
pub mod chat;
pub mod code_editor;
pub mod deliver;
pub mod editor_settings;
pub mod inspector;
pub mod lettering;
pub mod library;
pub mod media_pool;
pub mod mixer;
pub mod notices;
pub mod scopes;
pub mod settings;
pub mod timeline;
pub mod transcript;
pub mod viewer;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;

/// Drag payload for pool media dropped on the timeline or viewer.
#[derive(Clone, Debug)]
pub struct MediaDrag {
    pub media: koo_core::MediaId,
    pub name: SharedString,
}

impl Render for MediaDrag {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        drag_chip(self.name.clone(), cx)
    }
}

/// Drag payload for a sound from the library, added to the pool when
/// dropped on the timeline or the chat.
#[derive(Clone, Debug)]
pub struct SoundDrag {
    pub sound: koo_core::LibraryId,
    pub name: SharedString,
}

impl Render for SoundDrag {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        drag_chip(self.name.clone(), cx)
    }
}

/// Drag payload for an FX library item: an effect dropped on a clip, a
/// transition on a cut or clip edge, a generator on a track. The chat takes
/// any of them as a reference.
#[derive(Clone, Debug)]
pub struct FxDrag {
    pub fx: koo_core::fx::FxRef,
    pub kind: koo_core::fx::FxKind,
    pub name: SharedString,
    /// A library program, copied into the project when dropped (`fx` is
    /// then a placeholder).
    pub library: Option<koo_core::LibraryId>,
}

impl FxDrag {
    /// The program to use where it is dropped, copying a library program
    /// into the project first.
    pub fn program(
        &self,
        state: &Entity<crate::state::EditorState>,
        cx: &mut App,
    ) -> Option<koo_core::fx::FxRef> {
        match self.library {
            Some(id) => state.update(cx, |s, cx| library::shared::use_program(s, id, cx)),
            None => Some(self.fx.clone()),
        }
    }
}

impl Render for FxDrag {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        drag_chip(self.name.clone(), cx)
    }
}

/// Drag payload for an audio effect from the library, dropped on an audio
/// clip or a track header.
#[derive(Clone, Debug)]
pub struct AudioFxDrag {
    pub kind: koo_core::audiofx::AudioFxKind,
    pub name: SharedString,
}

impl Render for AudioFxDrag {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        drag_chip(self.name.clone(), cx)
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let label = match self.refs.as_slice() {
            [(_, one)] => one.clone(),
            many => format!("{} items", many.len()).into(),
        };
        drag_chip(label, cx)
    }
}

fn drag_chip(label: SharedString, cx: &App) -> Div {
    div()
        .px_2()
        .py_1()
        .rounded_md()
        .text_xs()
        .bg(cx.theme().secondary)
        .child(label)
}
