//! Persistence. Projects are `.koo` files the user saves anywhere
//! ([`project_file`]), with debounced crash-recovery snapshots through
//! [`Autosaver`]. Everything else belongs to the user, not a project:
//! settings, style profiles, source facts, recipes and recent files, in
//! SQLite under `koo_core::paths::database()`.
//!
//! One [`Store`] wraps one connection. It is `Send`, so it can move to a
//! background thread; open one per thread when several need it (WAL lets
//! readers and one writer work side by side).
//!
//! Memory is read in small pieces: every list call takes an
//! offset and a limit, so the agent never has to load a whole profile.

mod autosave;
mod facts;
mod notes;
mod profiles;
pub mod project_file;
mod recent;
mod recipes;
mod render_cache;
mod schema;
mod settings;

pub use anyhow::{Error, Result};
pub use autosave::{Autosaver, DEBOUNCE, MAX_DELAY, Snapshot};
pub use facts::FactFilter;
pub use notes::parse_notes;
pub use profiles::{ProfileSummary, RuleFilter};
pub use project_file::ProjectFile;
pub use recipes::builtin_recipes;
pub use render_cache::RenderCacheSettings;
pub use settings::SettingsOverrides;

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Handle to the user database.
pub struct Store {
    conn: Connection,
    path: PathBuf,
}

impl Store {
    /// The user database at `koo_core::paths::database()`.
    pub fn open() -> Result<Self> {
        Self::open_at(&koo_core::paths::database())
    }

    pub fn open_at(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(Duration::from_secs(5))?;
        let mut store = Self {
            conn,
            path: path.to_path_buf(),
        };
        schema::migrate(&mut store.conn)?;
        recipes::seed(&store.conn)?;
        Ok(store)
    }

    /// Another connection to the same database, e.g. for a worker thread.
    pub fn reopen(&self) -> Result<Self> {
        Self::open_at(&self.path)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Schema version (`PRAGMA user_version`).
    pub fn schema_version(&self) -> Result<u32> {
        Ok(self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))?)
    }

    // --------------------------------------------------------- key / value

    fn meta<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let text: Option<String> = self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()?;
        Ok(match text {
            Some(t) => Some(serde_json::from_str(&t)?),
            None => None,
        })
    }

    fn set_meta<T: Serialize + ?Sized>(&self, key: &str, value: &T) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![key, serde_json::to_string(value)?],
        )?;
        Ok(())
    }
}

/// Unix seconds.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Enum to its serde name (`RuleStatus::Proposed` → `proposed`), for
/// indexed columns.
fn tag<T: Serialize>(value: &T) -> Result<String> {
    match serde_json::to_value(value)? {
        serde_json::Value::String(s) => Ok(s),
        other => anyhow::bail!("not a unit enum: {other}"),
    }
}

fn untag<T: DeserializeOwned>(text: &str) -> Result<T> {
    Ok(serde_json::from_value(serde_json::Value::String(
        text.to_owned(),
    ))?)
}

fn uuid_text(id: &uuid::Uuid) -> String {
    id.hyphenated().to_string()
}

fn parse_uuid(text: &str) -> Result<uuid::Uuid> {
    Ok(uuid::Uuid::parse_str(text)?)
}

/// `LIMIT` value for an optional limit (`-1` = no limit in SQLite).
fn sql_limit(limit: Option<usize>) -> i64 {
    limit.map(|l| l as i64).unwrap_or(-1)
}
