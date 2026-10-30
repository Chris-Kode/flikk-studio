//! Recently opened project files, newest first.

use crate::Store;
use anyhow::Result;
use std::path::{Path, PathBuf};

const RECENT: &str = "recent_projects";
const KEPT: usize = 10;

impl Store {
    pub fn recent_projects(&self) -> Result<Vec<PathBuf>> {
        Ok(self.meta(RECENT)?.unwrap_or_default())
    }

    /// Put `path` first.
    pub fn add_recent_project(&self, path: &Path) -> Result<()> {
        let mut recent = self.recent_projects()?;
        recent.retain(|p| p != path);
        recent.insert(0, path.to_path_buf());
        recent.truncate(KEPT);
        self.set_meta(RECENT, &recent)
    }

    pub fn remove_recent_project(&self, path: &Path) -> Result<()> {
        let mut recent = self.recent_projects()?;
        recent.retain(|p| p != path);
        self.set_meta(RECENT, &recent)
    }
}
