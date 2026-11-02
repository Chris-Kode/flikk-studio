//! Deliver preferences, per user.

use crate::Store;
use anyhow::Result;

/// The render encoder choice (`auto`, `hardware`, `software`).
const RENDER_ENCODER: &str = "render_encoder";

impl Store {
    /// The encoder picked in the Deliver workspace; `None` until picked.
    pub fn render_encoder(&self) -> Result<Option<String>> {
        self.meta(RENDER_ENCODER)
    }

    pub fn save_render_encoder(&self, key: &str) -> Result<()> {
        self.set_meta(RENDER_ENCODER, key)
    }
}
