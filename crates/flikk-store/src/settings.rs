//! Agent settings: one set per user, plus per-project overrides for
//! everything except `backend` and `models` (PRD "Agent configuration"),
//! kept in the project's `.flikk` file.

use crate::{Store, recipes};
use anyhow::Result;
use flikk_core::agent::{AgentSettings, VisionSettings};
use flikk_core::bus::Permissions;
use flikk_core::cloud::CloudSettings;
use flikk_core::ids::{RecipeId, RuleId};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

const USER_SETTINGS: &str = "agent_settings";
/// The models the user picked over each task's default, by id.
const MODEL_PICKS: &str = "model_picks";
/// The provider picked for each capability (never its key).
const CLOUD_SETTINGS: &str = "cloud_settings";

/// Per-project overrides. `None` inherits the user setting. The backend is
/// per user only.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SettingsOverrides {
    pub permissions: Option<Permissions>,
    pub instructions: Option<String>,
    pub enabled_recipes: Option<Vec<RecipeId>>,
    pub vision: Option<VisionSettings>,
    /// `Some(None)` forces "follow the source" over a user language.
    pub language: Option<Option<String>>,
    pub style_profile: Option<Option<RuleId>>,
    pub web: Option<bool>,
    pub image_generation: Option<bool>,
}

impl SettingsOverrides {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub fn apply(&self, user: &AgentSettings) -> AgentSettings {
        let mut s = user.clone();
        if let Some(v) = &self.permissions {
            s.permissions = v.clone();
        }
        if let Some(v) = &self.instructions {
            s.instructions = v.clone();
        }
        if let Some(v) = &self.enabled_recipes {
            s.enabled_recipes = v.clone();
        }
        if let Some(v) = &self.vision {
            s.vision = v.clone();
        }
        if let Some(v) = &self.language {
            s.language = v.clone();
        }
        if let Some(v) = &self.style_profile {
            s.style_profile = *v;
        }
        if let Some(v) = self.web {
            s.web = v;
        }
        if let Some(v) = self.image_generation {
            s.image_generation = v;
        }
        s
    }

    /// The overrides that turn `user` into `effective` (fields that differ).
    /// `effective.backend` is ignored.
    pub fn diff(user: &AgentSettings, effective: &AgentSettings) -> Self {
        fn changed<T: Clone + PartialEq>(user: &T, effective: &T) -> Option<T> {
            (user != effective).then(|| effective.clone())
        }
        Self {
            permissions: changed(&user.permissions, &effective.permissions),
            instructions: changed(&user.instructions, &effective.instructions),
            enabled_recipes: changed(&user.enabled_recipes, &effective.enabled_recipes),
            vision: changed(&user.vision, &effective.vision),
            language: changed(&user.language, &effective.language),
            style_profile: changed(&user.style_profile, &effective.style_profile),
            web: changed(&user.web, &effective.web),
            image_generation: changed(&user.image_generation, &effective.image_generation),
        }
    }
}

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

    /// Save settings edited in the UI, where `effective` is the merged view:
    /// fields the project overrides update its overrides, the rest (and the
    /// backend) update the user settings. Returns the project's overrides.
    pub fn save_effective_settings(
        &self,
        overrides: &SettingsOverrides,
        effective: &AgentSettings,
    ) -> Result<SettingsOverrides> {
        let old_user = self.user_settings()?;
        let mut user = effective.clone();
        let mut next = SettingsOverrides::default();
        macro_rules! split {
            ($($field:ident),*) => {$(
                if overrides.$field.is_some() {
                    user.$field = old_user.$field.clone();
                    next.$field = Some(effective.$field.clone());
                }
            )*};
        }
        split!(
            permissions,
            instructions,
            enabled_recipes,
            vision,
            language,
            style_profile,
            web,
            image_generation
        );
        self.save_user_settings(&user)?;
        Ok(next)
    }

    /// Effective settings for a project: user settings with its overrides.
    pub fn load_settings(&self, overrides: &SettingsOverrides) -> Result<AgentSettings> {
        Ok(overrides.apply(&self.user_settings()?))
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
