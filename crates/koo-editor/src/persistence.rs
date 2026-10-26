//! Load, autosave and settings. Owned by the store work stream; backed by
//! `koo-store` under `koo_core::paths::home()`.

use koo_core::agent::AgentSettings;
use koo_core::bus::CommandBus;
use koo_core::model::Project;

pub fn load_or_new() -> (Project, AgentSettings) {
    (Project::new("Untitled"), AgentSettings::default())
}

pub fn autosave(_bus: &CommandBus) {}

pub fn save_settings(_settings: &AgentSettings) {}
