//! Agent settings: one set per user, for every project (PRD "Agent
//! configuration").

use crate::{Store, recipes};
use anyhow::Result;
use flikk_core::agent::AgentSettings;
use flikk_core::cloud::CloudSettings;
use serde::Serialize;
use serde::de::DeserializeOwned;

const USER_SETTINGS: &str = "agent_settings";
/// The models the user picked over each task's default, by id.
const MODEL_PICKS: &str = "model_picks";
/// The provider picked for each capability (never its key).
const CLOUD_SETTINGS: &str = "cloud_settings";

impl Store {
    /// User settings. First run: defaults with every built-in recipe on.
    pub fn user_settings(&self) -> Result<AgentSettings> {
        let stored: Option<serde_json::Value> = self.meta(USER_SETTINGS)?;
        Ok(match stored {
            Some(value) => lenient(value),
            None => AgentSettings {
                enabled_recipes: recipes::builtin_ids(),
                ..AgentSettings::default()
            },
        })
    }

    pub fn save_user_settings(&self, settings: &AgentSettings) -> Result<()> {
        self.set_meta(USER_SETTINGS, settings)
    }

    /// Ids of the models the user picked in Settings → Integrations, per user.
    pub fn model_picks(&self) -> Result<Vec<String>> {
        Ok(self.meta(MODEL_PICKS)?.unwrap_or_default())
    }

    pub fn save_model_picks(&self, ids: &[&str]) -> Result<()> {
        self.set_meta(MODEL_PICKS, ids)
    }

    /// The providers set in Settings → Integrations, per user.
    pub fn cloud_settings(&self) -> Result<CloudSettings> {
        Ok(self
            .meta::<serde_json::Value>(CLOUD_SETTINGS)?
            .map(lenient)
            .unwrap_or_default())
    }

    pub fn save_cloud_settings(&self, settings: &CloudSettings) -> Result<()> {
        self.set_meta(CLOUD_SETTINGS, settings)
    }
}

/// Deserialize over the defaults, so settings saved by an older build
/// (missing fields) still load instead of being reset. A stored value that
/// no longer fits (a renamed enum variant, a hand edit) falls back on its
/// own: everything else stored is kept, so the next save loses only that.
fn lenient<T: Serialize + DeserializeOwned + Default>(stored: serde_json::Value) -> T {
    let mut base = serde_json::to_value(T::default()).unwrap_or_default();
    overlay(&mut base, &mut Vec::new(), stored, &|v| {
        T::deserialize(v).is_ok()
    });
    T::deserialize(&base).unwrap_or_default()
}

/// Merges `over` into `root` at `path` when the result still passes `fits`;
/// otherwise goes down `over`'s keys and keeps each one that does. A list
/// keeps the items that still fit, so one stale id doesn't empty
/// `enabled_recipes` (empty turns every recipe on); with none left it keeps
/// the default.
fn overlay(
    root: &mut serde_json::Value,
    path: &mut Vec<String>,
    over: serde_json::Value,
    fits: &dyn Fn(&serde_json::Value) -> bool,
) {
    let mut candidate = root.clone();
    merge(slot(&mut candidate, path), over.clone());
    if fits(&candidate) {
        *root = candidate;
        return;
    }
    let over = match over {
        serde_json::Value::Object(over) => over,
        serde_json::Value::Array(items) => {
            let mut kept = Vec::new();
            for item in items {
                kept.push(item);
                let mut candidate = root.clone();
                *slot(&mut candidate, path) = serde_json::Value::Array(kept.clone());
                if !fits(&candidate) {
                    kept.pop();
                }
            }
            if !kept.is_empty() {
                *slot(root, path) = serde_json::Value::Array(kept);
            }
            return;
        }
        _ => return,
    };
    let mut candidate = root.clone();
    let at = slot(&mut candidate, path);
    if !at.is_object() {
        *at = serde_json::Value::Object(Default::default());
        if !fits(&candidate) {
            return;
        }
        *root = candidate;
    }
    for (k, v) in over {
        path.push(k);
        overlay(root, path, v, fits);
        path.pop();
    }
}

/// The value at `path`, made (as `null`) where missing. Only called on
/// paths whose parents are objects.
fn slot<'a>(mut value: &'a mut serde_json::Value, path: &[String]) -> &'a mut serde_json::Value {
    for k in path {
        value = match value {
            serde_json::Value::Object(map) => {
                map.entry(k.clone()).or_insert(serde_json::Value::Null)
            }
            other => other,
        };
    }
    value
}

fn merge(base: &mut serde_json::Value, over: serde_json::Value) {
    match (base, over) {
        (serde_json::Value::Object(b), serde_json::Value::Object(o)) => {
            for (k, v) in o {
                match b.get_mut(&k) {
                    Some(slot) => merge(slot, v),
                    None => {
                        b.insert(k, v);
                    }
                }
            }
        }
        (slot, v) => *slot = v,
    }
}
