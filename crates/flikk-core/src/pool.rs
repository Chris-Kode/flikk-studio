//! Media items for files made or fetched for a project, and the commands
//! that put one in the project's pool, in a bin of its own.

use crate::cloud::{Capability, Generated};
use crate::command::Command;
use crate::ids::{BinId, MediaId};
use crate::model::{Bin, MediaItem, MediaKind, Project};
use std::path::PathBuf;

/// Name of the bin generated `item` goes into.
pub fn generated_bin(item: &MediaItem) -> &'static str {
    if item.kind == MediaKind::Image {
        return "Images Generated";
    }
    match item.generated.as_deref().map(|g| g.capability) {
        Some(Capability::SoundEffects) => "Sound Effects Generated",
        Some(Capability::Voiceover) => "Voiceover Generated",
        Some(Capability::Video) => "Video Generated",
        _ => "Music Generated",
    }
}

/// The media item for `generated`, probed and fingerprinted by `media`
/// when there is one; not in a bin yet ([`pool_commands`] puts it in one).
pub fn media_item(
    generated: &Generated,
    media: Option<&dyn crate::services::MediaService>,
) -> MediaItem {
    let kind = match generated.info.capability {
        Capability::Video => MediaKind::Video,
        Capability::Image => MediaKind::Image,
        _ => MediaKind::Audio,
    };
    MediaItem {
        id: MediaId::new(),
        path: generated.path.clone(),
        kind,
        fingerprint: media.and_then(|m| m.fingerprint(&generated.path).ok()),
        info: media.and_then(|m| m.probe(&generated.path).ok()),
        bin: None,
        tags: Vec::new(),
        proxy: None,
        generated: Some(Box::new(generated.info.clone())),
        name: None,
        tone_map: Default::default(),
    }
}

/// `path`, a picture an agent backend made with its own tool, as an item.
/// Not in a bin yet; with no provider, it has no
/// [`GenerationInfo`](crate::cloud::GenerationInfo).
pub fn image_item(path: PathBuf, media: Option<&dyn crate::services::MediaService>) -> MediaItem {
    MediaItem {
        id: MediaId::new(),
        kind: MediaKind::Image,
        fingerprint: media.and_then(|m| m.fingerprint(&path).ok()),
        info: media.and_then(|m| m.probe(&path).ok()),
        path,
        bin: None,
        tags: Vec::new(),
        proxy: None,
        generated: None,
        name: None,
        tone_map: Default::default(),
    }
}

/// `path`, a sample of a voice, as an item to listen to: not in any pool.
pub fn sample_item(path: PathBuf, media: Option<&dyn crate::services::MediaService>) -> MediaItem {
    MediaItem {
        id: MediaId::new(),
        kind: MediaKind::Audio,
        fingerprint: None,
        info: media.and_then(|m| m.probe(&path).ok()),
        path,
        bin: None,
        tags: Vec::new(),
        proxy: None,
        generated: None,
        name: None,
        tone_map: Default::default(),
    }
}

/// The commands that add `item` to `project`'s pool, in the bin named
/// `name`, made first if the project has none.
pub fn pool_commands(project: &Project, mut item: MediaItem, name: &str) -> Vec<Command> {
    let mut commands = Vec::new();
    let bin = match project
        .bins
        .iter()
        .find(|b| b.parent.is_none() && b.name == name)
    {
        Some(bin) => bin.id,
        None => {
            let bin = Bin {
                id: BinId::new(),
                name: name.into(),
                parent: None,
            };
            let id = bin.id;
            let mut bins = project.bins.clone();
            bins.push(bin);
            commands.push(Command::SetBins { bins });
            id
        }
    };
    item.bin = Some(bin);
    commands.push(Command::ImportMedia { media: item });
    commands
}
