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
/// (missing fields) still load instead of being reset.
fn lenient<T: Serialize + DeserializeOwned + Default>(stored: serde_json::Value) -> T {
    let mut base = serde_json::to_value(T::default()).unwrap_or_default();
    merge(&mut base, stored);
    serde_json::from_value(base).unwrap_or_default()
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
