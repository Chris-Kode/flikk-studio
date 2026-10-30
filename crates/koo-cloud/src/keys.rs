//! API keys, one per provider, in the system's credential store (macOS
//! Keychain, Windows Credential Manager, the Secret Service on Linux) under
//! the service "koo-editor". Where the system has none, they go in an
//! owner-only `credentials.json` in the user-data folder. A provider's
//! environment variable ([`Provider::key_env`]) wins over both. Keys are
//! never written to the database or to project files.

use crate::{Provider, lock};
use koo_core::paths;
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

const SERVICE: &str = "koo-editor";

/// Where a provider's key comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeySource {
    /// Its environment variable; it can't be changed from the editor.
    Environment,
    Stored,
    None,
}

/// The keys read so far; the credential store is asked once per provider.
#[derive(Default)]
pub struct Keys {
    read: Mutex<HashMap<&'static str, Option<String>>>,
}

impl Keys {
    /// `provider`'s key. The first call may wait on the credential store
    /// (and on macOS ask the user to allow it), so make it off the UI
    /// thread.
    pub fn get(&self, provider: &Provider) -> Option<String> {
        if let Some(key) = from_env(provider) {
            return Some(key);
        }
        if let Some(known) = lock(&self.read).get(provider.id) {
            return known.clone();
        }
        let stored = load(provider.id);
        lock(&self.read).insert(provider.id, stored.clone());
        stored
    }

    pub fn source(&self, provider: &Provider) -> KeySource {
        if from_env(provider).is_some() {
            KeySource::Environment
        } else if self.get(provider).is_some() {
            KeySource::Stored
        } else {
            KeySource::None
        }
    }

    /// Keep `key` for `provider`, replacing the one stored.
    pub fn set(&self, provider: &Provider, key: &str) -> Result<(), String> {
        let key = key.trim();
        if key.is_empty() {
            return self.remove(provider);
        }
        save(provider.id, Some(key))?;
        lock(&self.read).insert(provider.id, Some(key.to_owned()));
        Ok(())
    }

    pub fn remove(&self, provider: &Provider) -> Result<(), String> {
        save(provider.id, None)?;
        lock(&self.read).insert(provider.id, None);
        Ok(())
    }
}

fn from_env(provider: &Provider) -> Option<String> {
    std::env::var(provider.key_env)
        .ok()
        .map(|k| k.trim().to_owned())
        .filter(|k| !k.is_empty())
}

/// The credential store's entry, else the file's.
fn load(id: &str) -> Option<String> {
    if let Ok(entry) = keyring::Entry::new(SERVICE, id)
        && let Ok(key) = entry.get_password()
    {
        return Some(key);
    }
    read_file().remove(id)
}

/// Into the credential store when there is one, else the file. Whichever
/// is not used loses any copy it had.
fn save(id: &str, key: Option<&str>) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE, id);
    let in_store = match (&entry, key) {
        (Ok(entry), Some(key)) => entry.set_password(key).is_ok(),
        (Ok(entry), None) => {
            let _ = entry.delete_credential();
            false
        }
        (Err(_), _) => false,
    };
    let mut file = read_file();
    let before = file.clone();
    match key {
        Some(key) if !in_store => {
            file.insert(id.to_owned(), key.to_owned());
        }
        _ => {
            file.remove(id);
        }
    }
    if file != before {
        write_file(&file)?;
    }
    Ok(())
}

fn read_file() -> BTreeMap<String, String> {
    std::fs::read_to_string(paths::credentials())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Written owner-only, like `bridge.json`.
fn write_file(keys: &BTreeMap<String, String>) -> Result<(), String> {
    let path = paths::credentials();
    if keys.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        };
    }
    let text = serde_json::to_string_pretty(keys).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.part");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let write = || -> std::io::Result<()> {
        use std::io::Write;
        let mut f = options.open(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, &path)
    };
    write().map_err(|e| format!("could not write {}: {e}", path.display()))
}
