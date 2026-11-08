//! The pool's context and button menus: items, selections, folders,
//! empty space, moving, and what a provider can generate.

use super::bins::{BinTree, bin_tree};
use super::{AnalyzeEntry, Entry, MediaPoolPanel, voiceover};
use crate::state::EditorState;
use crate::ui::kit::{Button, DropdownMenu as _, IconName, PopupMenu, PopupMenuItem};
use crate::ui::library::shared;
use flikk_core::command::InsertMode;
use flikk_core::model::MediaKind;
use flikk_core::{BinId, MediaId};
use gpui_kit::*;

/// The menu of item `id`, from its "⋯" button or a right click; built as
/// it opens, so what it offers is current.
pub(super) fn item_menu(
    menu: PopupMenu,
    panel: &Entity<MediaPoolPanel>,
    id: MediaId,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let (group, folders) = panel.read(cx).split_of(Entry::Media(id));
    if group.len() + folders.len() > 1 {
        return selection_menu(menu, panel, id, group, folders, window, cx);
    }
    let mut menu = menu;
    let state = panel.read(cx).state.clone();
    if state.read(cx).is_offline(id) {
        menu = menu
            .item(
                PopupMenuItem::new("Relink…")
                    .icon(IconName::Unlink)
                    .on_click(move |_, _, cx| crate::relink::relink_item(&state, id, cx)),
            )
            .separator();
    }
    if let Some(entry) = panel.read(cx).analyze_entry(id, cx) {
        let panel = panel.clone();
        menu = menu.item(match entry {
            AnalyzeEntry::Queued => PopupMenuItem::new("Queued for analysis").disabled(true),
            AnalyzeEntry::Running => PopupMenuItem::new("Analyzing…").disabled(true),
            AnalyzeEntry::Unreadable => {
                PopupMenuItem::new("Can’t analyze this file").disabled(true)
            }
            AnalyzeEntry::SetUp => {
                PopupMenuItem::new("Set up analysis…").on_click(|_, window, cx| {
                    window.dispatch_action(Box::new(crate::app::OpenIntegrationsSettings), cx)
                })
            }
            AnalyzeEntry::Ready { again } => {
                let label = if again { "Analyze again" } else { "Analyze" };
                PopupMenuItem::new(label).on_click(move |_, _, cx| {
                    panel.update(cx, |this, cx| this.analyze(id, again, cx))
                })
            }
        });
        menu = menu.separator();
    }
    let (open, remove) = (panel.clone(), panel.clone());
    let state = panel.read(cx).state.clone();
    let sound = state.read(cx).bus.project().media(id);
    let sound = sound
        .filter(|item| state.read(cx).can_listen(item))
        .is_some();
    let open_label = match sound {
        true if listening_to(&state, id, cx) => "Pause",
        true => "Play",
        false => "Open in viewer",
    };
    let rename = panel.clone();
    let mut menu = menu
        .item(
            PopupMenuItem::new(open_label).on_click(move |_, window, cx| {
                open.update(cx, |this, cx| this.select(id, true, window, cx))
            }),
        )
        .item(PopupMenuItem::new("Rename").on_click(move |_, window, cx| {
            rename.update(cx, |this, cx| this.start_item_rename(id, window, cx))
        }));
    let edits = |label: &'static str, mode: InsertMode| {
        let (p, group) = (panel.clone(), group.clone());
        PopupMenuItem::new(label).on_click(move |_, _, cx| {
            let ids = p.read(cx).in_pool_order(&group, cx);
            let state = p.read(cx).state.clone();
            state.update(cx, |s, cx| s.edit_at_playhead(&ids, mode, cx));
        })
    };
    menu = menu
        .separator()
        .item(edits("Insert at playhead", InsertMode::Insert))
        .item(edits("Overwrite at playhead", InsertMode::Overwrite));
    let path = state
        .read(cx)
        .bus
        .project()
        .media(id)
        .map(|m| m.path.clone());
    if let Some(path) = path.filter(|p| p.exists()) {
        menu = menu.item(
            PopupMenuItem::new(crate::ui::deliver::REVEAL)
                .on_click(move |_, _, cx| cx.reveal_path(&path)),
        );
    }
    let menu = move_menu(menu.separator(), panel, &group, &folders, window, cx);
    let menu = if shared::can_save_sound(state.read(cx), id, cx) {
        menu.item(
            PopupMenuItem::new("Add to Library")
                .on_click(move |_, _, cx| state.update(cx, |s, cx| shared::save_sound(s, id, cx))),
        )
    } else {
        menu
    };
    let menu = match voiceover_take(panel, id, cx) {
        Some(take) => {
            let p = panel.clone();
            let state = panel.read(cx).state.clone();
            let saved = voice_saved(&state, &take, cx);
            let menu = menu.item(PopupMenuItem::new("Regenerate…").on_click(
                move |_, window, cx| {
                    let take = Some(take.clone());
                    p.update(cx, |this, cx| this.open_voiceover(take, window, cx))
                },
            ));
            if saved {
                menu
            } else {
                let s = state.clone();
                menu.item(
                    PopupMenuItem::new("Save Voice to Library")
                        .on_click(move |_, _, cx| s.update(cx, |s, cx| save_voice(s, id, cx))),
                )
            }
        }
        None => menu,
    };
    // A picture can be edited, or followed, by the model making pictures.
    let picture = panel
        .read(cx)
        .state
        .read(cx)
        .bus
        .project()
        .media(id)
        .is_some_and(|m| m.kind == MediaKind::Image);
    let menu = match panel.read(cx).image_model(cx).filter(|_| picture) {
        Some(model) => {
            let p = panel.clone();
            menu.item(
                PopupMenuItem::new("Generate image…").on_click(move |_, window, cx| {
                    let model = model.clone();
                    p.update(cx, |this, cx| this.open_image(model, Some(id), window, cx))
                }),
            )
        }
        None => menu,
    };
    menu.separator().item(
        PopupMenuItem::new("Remove from project")
            .icon(IconName::Trash)
            .destructive()
            .on_click(move |_, window, cx| {
                let (group, folders) = (group.clone(), folders.clone());
                remove.update(cx, |this, cx| {
                    this.confirm_delete(group, folders, window, cx)
                })
            }),
    )
}

/// The menu on several entries: only what applies to every one of them.
/// Clips can go on the timeline and sync; folders only move or go.
fn selection_menu(
    menu: PopupMenu,
    panel: &Entity<MediaPoolPanel>,
    lead: MediaId,
    media: Vec<MediaId>,
    folders: Vec<BinId>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    if !folders.is_empty() {
        return common_menu(menu, panel, media, folders, window, cx);
    }
    let mut menu = menu;
    // Analyze when every clip can: each runs fresh or again as it would
    // alone, and "again" only when every one has whole results.
    let ready: Option<Vec<(MediaId, bool)>> = media
        .iter()
        .map(|m| match panel.read(cx).analyze_entry(*m, cx) {
            Some(AnalyzeEntry::Ready { again }) => Some((*m, again)),
            _ => None,
        })
        .collect();
    if let Some(ready) = ready {
        let label = match ready.iter().all(|(_, again)| *again) {
            true => "Analyze again",
            false => "Analyze",
        };
        let p = panel.clone();
        menu = menu
            .item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                p.update(cx, |this, cx| {
                    for (id, again) in &ready {
                        this.analyze(*id, *again, cx);
                    }
                })
            }))
            .separator();
    }
    let state = panel.read(cx).state.clone();
    let edits = |label: &'static str, mode: InsertMode| {
        let (p, media) = (panel.clone(), media.clone());
        PopupMenuItem::new(label).on_click(move |_, _, cx| {
            let ids = p.read(cx).in_pool_order(&media, cx);
            let state = p.read(cx).state.clone();
            state.update(cx, |s, cx| s.edit_at_playhead(&ids, mode, cx));
        })
    };
    menu = menu
        .item(edits("Insert at playhead", InsertMode::Insert))
        .item(edits("Overwrite at playhead", InsertMode::Overwrite));
    if let Some((group, recorder)) = panel.read(cx).sync_group(lead, cx) {
        let item = |label: &'static str, mute: bool| {
            let (state, group) = (state.clone(), group.clone());
            PopupMenuItem::new(label).on_click(move |_, _, cx| {
                let group = group.clone();
                state.update(cx, |s, cx| s.sync_media(group, mute, cx))
            })
        };
        menu = menu.separator().item(item("Sync", false));
        if recorder {
            menu = menu.item(item("Sync, Recorder Sound", true));
        }
    }
    common_menu(menu.separator(), panel, media, folders, window, cx)
}

/// What any selection can do: move, and leave the project.
fn common_menu(
    menu: PopupMenu,
    panel: &Entity<MediaPoolPanel>,
    media: Vec<MediaId>,
    folders: Vec<BinId>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let menu = move_menu(menu, panel, &media, &folders, window, cx);
    let label = format!("Remove {} items from project", media.len() + folders.len());
    let remove = panel.clone();
    menu.separator().item(
        PopupMenuItem::new(label)
            .icon(IconName::Trash)
            .destructive()
            .on_click(move |_, window, cx| {
                let (media, folders) = (media.clone(), folders.clone());
                remove.update(cx, |this, cx| {
                    this.confirm_delete(media, folders, window, cx)
                })
            }),
    )
}

/// "Move to", opening the folders `media` and `folders` can go to (not
/// into a folder that moves); ticks the one they all sit in.
fn move_menu(
    menu: PopupMenu,
    panel: &Entity<MediaPoolPanel>,
    media: &[MediaId],
    folders: &[BinId],
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let project = panel.read(cx).state.read(cx).bus.project();
    let tree = BinTree::of(&project.bins);
    let bins: Vec<(Option<BinId>, String, usize)> = std::iter::once((None, "Media".into(), 0))
        .chain(
            bin_tree(&project.bins)
                .into_iter()
                .filter(|(b, _)| !folders.iter().any(|f| tree.within(Some(b.id), *f)))
                .map(|(b, depth)| (Some(b.id), b.name.clone(), depth + 1)),
        )
        .collect();
    if bins.len() < 2 && folders.is_empty() {
        return menu;
    }
    let mut homes = media
        .iter()
        .filter_map(|m| project.media(*m))
        .map(|m| tree.home(m.bin))
        .chain(
            project
                .bins
                .iter()
                .filter(|b| folders.contains(&b.id))
                .map(|b| tree.home(b.parent)),
        );
    let first = homes.next().flatten();
    let shared = homes.all(|b| b == first).then_some(first);
    let (panel, media, folders) = (panel.clone(), media.to_vec(), folders.to_vec());
    menu.submenu("Move to", window, cx, move |mut menu, _, _| {
        for (bin, name, depth) in &bins {
            let (panel, media, folders, bin) =
                (panel.clone(), media.clone(), folders.clone(), *bin);
            let indent = "    ".repeat(depth.saturating_sub(1));
            menu = menu.item(
                PopupMenuItem::new(format!("{indent}{name}"))
                    .checked(shared == Some(bin))
                    .on_click(move |_, _, cx| {
                        panel.update(cx, |this, cx| this.move_entries(&media, &folders, bin, cx))
                    }),
            );
        }
        menu
    })
}

/// What a provider can make, each opening its form: the Generate
/// button's menu and the Generate submenu.
pub(super) fn generate_items(
    mut menu: PopupMenu,
    panel: &Entity<MediaPoolPanel>,
    cx: &App,
) -> PopupMenu {
    let pool = panel.read(cx);
    let (generate, video, image, can_voiceover) = (
        pool.can_generate(cx),
        pool.video_model(cx),
        pool.image_model(cx),
        pool.can_voiceover(cx),
    );
    for kind in generate {
        let panel = panel.clone();
        menu = menu.item(
            PopupMenuItem::new(format!("{}…", kind.label()))
                .icon(kind.icon())
                .on_click(move |_, window, cx| {
                    panel.update(cx, |this, cx| this.open_generate(kind, window, cx))
                }),
        );
    }
    if let Some(model) = video {
        let p = panel.clone();
        menu = menu.item(PopupMenuItem::new("Video…").icon(IconName::Video).on_click(
            move |_, window, cx| {
                let model = model.clone();
                p.update(cx, |this, cx| this.open_video(model, window, cx))
            },
        ));
    }
    if let Some(model) = image {
        let p = panel.clone();
        menu = menu.item(PopupMenuItem::new("Image…").icon(IconName::Image).on_click(
            move |_, window, cx| {
                let model = model.clone();
                p.update(cx, |this, cx| this.open_image(model, None, window, cx))
            },
        ));
    }
    if can_voiceover {
        let p = panel.clone();
        menu = menu.item(
            PopupMenuItem::new("Voiceover…")
                .icon(IconName::Mic)
                .on_click(move |_, window, cx| {
                    p.update(cx, |this, cx| this.open_voiceover(None, window, cx))
                }),
        );
        let p = panel.clone();
        menu =
            menu.separator().item(
                PopupMenuItem::new("Voices…").on_click(move |_, window, cx| {
                    let state = p.read(cx).state.clone();
                    crate::ui::voices::open(
                        state,
                        crate::ui::voices::Start::Find,
                        None,
                        window,
                        cx,
                    );
                }),
            );
    }
    menu
}

/// The menu on a right click between items.
pub(super) fn space_menu(
    menu: PopupMenu,
    panel: &Entity<MediaPoolPanel>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let (folder, import) = (panel.clone(), panel.clone());
    let can_make = panel.read(cx).can_make(cx);
    let menu = menu
        .item(
            PopupMenuItem::new("New folder")
                .icon(IconName::FolderPlus)
                .on_click(move |_, window, cx| {
                    folder.update(cx, |this, cx| this.new_bin(this.bin, window, cx))
                }),
        )
        .item(
            PopupMenuItem::new("Import…")
                .icon(IconName::Download)
                .on_click(move |_, _, cx| import.update(cx, |this, cx| this.prompt_import(cx))),
        );
    if !can_make {
        return menu;
    }
    let panel = panel.clone();
    menu.submenu("Generate", window, cx, move |menu, _, cx| {
        generate_items(menu, &panel, cx)
    })
}

/// A folder's "⋯", shown on hover, opening its menu. A press on it
/// stays with it: the folder underneath would open.
pub(super) fn folder_more(id: BinId, panel: &Entity<MediaPoolPanel>) -> Div {
    let panel = panel.clone();
    div()
        .flex_none()
        .opacity(0.)
        .group_hover(format!("folder-{id}"), |s| s.opacity(1.))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            Button::new(ElementId::Name(format!("folder-menu-{id}").into()))
                .ghost()
                .xsmall()
                .icon(IconName::Ellipsis)
                .dropdown_menu(move |menu, window, cx| bin_menu(menu, &panel, id, window, cx)),
        )
}

/// A folder's menu, from its "⋯" or a right click: for the selection when
/// the folder is in it.
pub(super) fn bin_menu(
    menu: PopupMenu,
    panel: &Entity<MediaPoolPanel>,
    id: BinId,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let (media, folders) = panel.read(cx).split_of(Entry::Folder(id));
    if media.len() + folders.len() > 1 {
        let lead = media.first().copied();
        return match lead {
            Some(lead) => selection_menu(menu, panel, lead, media, folders, window, cx),
            None => common_menu(menu, panel, media, folders, window, cx),
        };
    }
    let rename = panel.clone();
    let menu = menu.item(PopupMenuItem::new("Rename").on_click(move |_, window, cx| {
        rename.update(cx, |this, cx| this.start_rename(id, window, cx))
    }));
    let menu = move_menu(menu, panel, &media, &folders, window, cx);
    let delete = panel.clone();
    menu.separator().item(
        PopupMenuItem::new("Delete folder")
            .icon(IconName::Trash)
            .destructive()
            .on_click(move |_, window, cx| {
                let (media, folders) = (media.clone(), folders.clone());
                delete.update(cx, |this, cx| {
                    this.confirm_delete(media, folders, window, cx)
                })
            }),
    )
}

/// The voiceover `id` is, when a provider can make it again.
fn voiceover_take(
    panel: &Entity<MediaPoolPanel>,
    id: MediaId,
    cx: &App,
) -> Option<voiceover::Take> {
    let panel = panel.read(cx);
    let state = panel.state.read(cx);
    let generated = state.bus.project().media(id)?.generated.as_deref()?;
    let reading = generated.reading.clone()?;
    panel.can_voiceover(cx).then(|| voiceover::Take {
        script: generated.prompt.clone(),
        reading,
    })
}

/// Whether the voice that read `take` is in the library.
fn voice_saved(state: &Entity<EditorState>, take: &voiceover::Take, cx: &App) -> bool {
    let state = state.read(cx);
    let provider = state.voice_provider().unwrap_or_default();
    state
        .library
        .voice_of(&provider, &take.reading.voice)
        .is_some()
}

/// Keep the voice that read voiceover `id` in the library, with the
/// voiceover as its sample.
fn save_voice(s: &mut EditorState, id: MediaId, cx: &mut Context<EditorState>) {
    let Some(item) = s.bus.project().media(id).cloned() else {
        return;
    };
    let Some(reading) = item.generated.as_deref().and_then(|g| g.reading.clone()) else {
        return;
    };
    let known = s
        .account_voices
        .voices
        .iter()
        .flatten()
        .find(|v| v.id == reading.voice)
        .cloned();
    let voice = known.unwrap_or_else(|| flikk_core::cloud::Voice {
        id: reading.voice.clone(),
        name: reading.voice_name.clone(),
        ..Default::default()
    });
    s.keep_voice(
        &voice,
        flikk_core::cloud::VoiceOrigin::Account,
        Some(&item),
        cx,
    );
    s.keep_reading(&reading, cx);
}

/// Whether pool item `id` is playing.
fn listening_to(state: &Entity<EditorState>, id: MediaId, cx: &App) -> bool {
    let state = state.read(cx);
    let item = state.bus.project().media(id);
    let preview = state.source.as_ref();
    item.zip(preview).is_some_and(|(item, p)| p.plays(item))
}
