//! Blockouts kept between calls. A scene is long to write and quick to
//! change, so one that was looked at or asked a video with stays on disk
//! under a short id: the agent changes it by saying what differs, the
//! user's changes on the request's card go into the same one, and a
//! second take starts from where the first was left instead of from
//! nothing.
//!
//! They live in the cache, the newest [`KEPT`] of them. One that is gone
//! is written again; nothing else is lost with it.

use flikk_core::blockout::Scene;
use flikk_core::paths;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// How many are kept; older ones go as new ones come.
const KEPT: usize = 40;

/// A blockout as it is kept.
#[derive(Serialize, Deserialize)]
pub struct Draft {
    pub scene: Scene,
    /// The picture it was made for, width over height.
    pub shape: f64,
}

fn folder() -> PathBuf {
    paths::cache().join("blockouts")
}

/// Where the blockout `id` is kept; `None` for what is no id, so a name
/// handed in can't reach outside the folder.
fn file(id: &str) -> Option<PathBuf> {
    let plain = !id.is_empty() && id.len() <= 16 && id.chars().all(|c| c.is_ascii_alphanumeric());
    plain.then(|| folder().join(format!("{id}.json")))
}

/// Keep `scene`, made for a picture `shape` wide to one high, and hand
/// back the id it is kept under. It has an id even when it could not be
/// written; finding it then fails.
pub fn keep(scene: &Scene, shape: f64) -> String {
    let id = format!("b{}", &uuid::Uuid::new_v4().simple().to_string()[..5]);
    put(&id, scene, shape);
    forget_oldest();
    id
}

/// Put `scene` in place of what `id` held.
pub fn put(id: &str, scene: &Scene, shape: f64) {
    let (Some(path), Ok(json)) = (file(id), serde_json::to_vec(&Draft::of(scene, shape))) else {
        return;
    };
    let _ = std::fs::create_dir_all(folder());
    let _ = flikk_os::fs::write_atomic(&path, &json);
}

/// The blockout kept as `id`, or why there is none, said for the agent.
pub fn find(id: &str) -> Result<Draft, String> {
    let gone = || {
        format!(
            "No blockout is kept as `{id}`: it was cleared away, or the id is wrong. Write the \
             scene again as `blockout`"
        )
    };
    let bytes = file(id).and_then(|path| std::fs::read(path).ok());
    bytes
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or_else(gone)
}

impl Draft {
    fn of(scene: &Scene, shape: f64) -> Self {
        Self {
            scene: scene.clone(),
            shape,
        }
    }
}

/// Remove all but the newest [`KEPT`].
fn forget_oldest() {
    let Ok(entries) = std::fs::read_dir(folder()) else {
        return;
    };
    let mut kept: Vec<_> = entries
        .flatten()
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .collect();
    kept.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    for (_, path) in kept.into_iter().skip(KEPT) {
        let _ = std::fs::remove_file(path);
    }
}
