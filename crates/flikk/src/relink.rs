//! Offline media: sources the project names but the disk no longer has
//! (moved, renamed, an unplugged drive). Relinking points them at the files
//! again: pick a folder and every offline item found in it by file name is
//! relinked, its fingerprint confirming the match when it has one.

use crate::state::EditorState;
use flikk_core::MediaId;
use flikk_core::command::Command;
use flikk_core::model::MediaItem;
use flikk_core::services::MediaService;
use gpui_kit::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// How deep and how wide a folder is searched.
const MAX_DEPTH: usize = 8;
const MAX_FILES: usize = 100_000;

/// Pick a folder and relink the offline items found in it.
pub fn relink_folder(state: &Entity<EditorState>, cx: &mut App) {
    let paths = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: None,
    });
    let state = state.clone();
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(paths))) = paths.await else {
            return;
        };
        if let Some(dir) = paths.into_iter().next() {
            cx.update(|cx| relink_in(&state, dir, None, cx));
        }
    })
    .detach();
}

/// Pick the file for offline item `media`; the other offline items are
/// then looked for beside it.
pub fn relink_item(state: &Entity<EditorState>, media: MediaId, cx: &mut App) {
    let paths = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: None,
    });
    let state = state.clone();
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(paths))) = paths.await else {
            return;
        };
        let Some(file) = paths.into_iter().next() else {
            return;
        };
        cx.update(|cx| {
            if let Some(dir) = file.parent().map(Path::to_path_buf) {
                relink_in(&state, dir, Some((media, file)), cx);
            }
        });
    })
    .detach();
}

/// Search `dir` off the UI thread for the offline items, then relink what
/// was found as one undo step. `chosen` is a file the user picked for one
/// item, taken as is.
fn relink_in(
    state: &Entity<EditorState>,
    dir: PathBuf,
    chosen: Option<(MediaId, PathBuf)>,
    cx: &mut App,
) {
    let s = state.read(cx);
    let offline: Vec<MediaItem> = s
        .bus
        .project()
        .media
        .iter()
        .filter(|m| s.offline.contains(&m.id))
        .cloned()
        .collect();
    let media = s.services.media.clone();
    let state = state.clone();
    let search = cx
        .background_executor()
        .spawn(async move { find(&dir, &offline, chosen, media) });
    cx.spawn(async move |cx| {
        let (found, missing) = search.await;
        state.update(cx, |s, cx| {
            let commands: Vec<Command> = found
                .into_iter()
                .filter_map(|(id, path)| {
                    let mut item = s.bus.project().media(id)?.clone();
                    item.path = path;
                    Some(Command::UpdateMedia { media: item })
                })
                .collect();
            let relinked = commands.len();
            if relinked > 0 {
                s.execute_batch(commands, "relink_media", cx);
            }
            s.status = Some(match (relinked, missing) {
                (0, _) => "No offline media found there".into(),
                (n, 0) => format!("Relinked {n}"),
                (n, m) => format!("Relinked {n} · {m} still offline"),
            });
            cx.notify();
        });
    })
    .detach();
}

/// Where each offline item is under `dir`, and how many stay missing.
fn find(
    dir: &Path,
    offline: &[MediaItem],
    chosen: Option<(MediaId, PathBuf)>,
    media: Option<Arc<dyn MediaService>>,
) -> (Vec<(MediaId, PathBuf)>, usize) {
    let files = files_by_name(dir);
    let mut found = Vec::new();
    for item in offline {
        if let Some((id, path)) = &chosen
            && *id == item.id
        {
            found.push((item.id, path.clone()));
            continue;
        }
        let Some(name) = item.path.file_name() else {
            continue;
        };
        let candidates = files
            .get(&name.to_string_lossy().to_lowercase())
            .map(Vec::as_slice)
            .unwrap_or_default();
        let matching = candidates
            .iter()
            .find(|path| match (&item.fingerprint, &media) {
                (Some(want), Some(media)) => media.fingerprint(path).is_ok_and(|fp| fp == *want),
                _ => true,
            });
        if let Some(path) = matching {
            found.push((item.id, path.clone()));
        }
    }
    let missing = offline.len() - found.len();
    (found, missing)
}

/// Files under `dir` by lowercase file name, skipping hidden folders.
fn files_by_name(dir: &Path) -> HashMap<String, Vec<PathBuf>> {
    let mut out: HashMap<String, Vec<PathBuf>> = HashMap::new();
    let mut stack = vec![(dir.to_path_buf(), 0)];
    let mut seen = 0;
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            match entry.file_type() {
                Ok(t) if t.is_dir() && depth < MAX_DEPTH => stack.push((path, depth + 1)),
                Ok(t) if t.is_file() => {
                    out.entry(name.to_lowercase()).or_default().push(path);
                    seen += 1;
                    if seen >= MAX_FILES {
                        return out;
                    }
                }
                _ => {}
            }
        }
    }
    out
}
