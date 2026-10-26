//! Agent settings: one set per user, plus per-project overrides for
//! everything except `backend` (PRD "Agent configuration").

use crate::{Store, recipes, uuid_text};
use anyhow::Result;
use koo_core::agent::{AgentSettings, VisionSettings};
use koo_core::bus::Permissions;
use koo_core::ids::{ProjectId, RecipeId, RuleId};
use rusqlite::{OptionalExtension, params};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

const USER_SETTINGS: &str = "agent_settings";

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

    pub fn project_overrides(&self, project: ProjectId) -> Result<SettingsOverrides> {
        let text: Option<Option<String>> = self
            .conn
            .query_row(
                "SELECT settings FROM projects WHERE id = ?1",
                [uuid_text(&project.0)],
                |r| r.get(0),
            )
            .optional()?;
        Ok(match text.flatten() {
            Some(t) => serde_json::from_str(&t).unwrap_or_default(),
            None => SettingsOverrides::default(),
        })
    }

    /// No-op if the project row does not exist.
    pub fn save_project_overrides(
        &self,
        project: ProjectId,
        overrides: &SettingsOverrides,
    ) -> Result<()> {
        let text = (!overrides.is_empty())
            .then(|| serde_json::to_string(overrides))
            .transpose()?;
        self.conn.execute(
            "UPDATE projects SET settings = ?2 WHERE id = ?1",
            params![uuid_text(&project.0), text],
        )?;
        Ok(())
    }

    /// Store `effective` as this project's settings: only the fields that
    /// differ from the user settings become overrides.
    pub fn save_project_settings(
        &self,
        project: ProjectId,
        effective: &AgentSettings,
    ) -> Result<()> {
        let user = self.user_settings()?;
        self.save_project_overrides(project, &SettingsOverrides::diff(&user, effective))
    }

    /// Save settings edited in the UI, where `effective` is the merged view:
    /// fields this project overrides update the override, the rest (and the
    /// backend) update the user settings.
    pub fn save_effective_settings(
        &self,
        project: ProjectId,
        effective: &AgentSettings,
    ) -> Result<()> {
        let old_user = self.user_settings()?;
        let old = self.project_overrides(project)?;
        let mut user = effective.clone();
        let mut overrides = SettingsOverrides::default();
        macro_rules! split {
            ($($field:ident),*) => {$(
                if old.$field.is_some() {
                    user.$field = old_user.$field.clone();
                    overrides.$field = Some(effective.$field.clone());
                }
            )*};
        }
        split!(
            permissions,
            instructions,
            enabled_recipes,
            vision,
            language,
            style_profile
        );
        let tx = self.conn.unchecked_transaction()?;
        self.save_user_settings(&user)?;
        self.save_project_overrides(project, &overrides)?;
        tx.commit()?;
        Ok(())
    }

    /// Effective settings for a project: user settings with its overrides.
    pub fn load_settings(&self, project: ProjectId) -> Result<AgentSettings> {
        Ok(self
            .project_overrides(project)?
            .apply(&self.user_settings()?))
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
