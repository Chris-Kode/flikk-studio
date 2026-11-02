//! Settings of the background render cache (Settings → Storage), per user.

use crate::Store;
use anyhow::Result;
use serde::{Deserialize, Serialize};

const RENDER_CACHE: &str = "render_cache";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RenderCacheSettings {
    /// Compose heavy ranges into the cache while the user is idle.
    pub background: bool,
    /// Disk the cached frames may use, in GB; the least recently used go.
    pub limit_gb: u32,
}

impl Default for RenderCacheSettings {
    fn default() -> Self {
        Self {
            background: true,
            limit_gb: 10,
        }
    }
}

impl Store {
    pub fn render_cache_settings(&self) -> Result<RenderCacheSettings> {
        Ok(self.meta(RENDER_CACHE)?.unwrap_or_default())
    }

    pub fn save_render_cache_settings(&self, settings: &RenderCacheSettings) -> Result<()> {
        self.set_meta(RENDER_CACHE, settings)
    }
}
