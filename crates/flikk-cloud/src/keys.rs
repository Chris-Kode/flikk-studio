//! API keys, one per provider, in the system's credential store (macOS
//! Keychain, Windows Credential Manager, the Secret Service on Linux) under
//! the service "flikk". Where the system has none, or it can't be reached
//! when a key is saved, they go in an owner-only `credentials.json` in the
//! user-data folder. A provider's environment variable
//! ([`Provider::key_env`]) wins over both. Keys are never written to the
//! database or to project files.
//!
//! The file is read first: a key is only written there when the credential
//! store wouldn't take it, and every key the store takes clears the file's,
//! so the file's is always the newer. A removal the store can't make leaves
//! an empty key in the file, which hides the store's until it can.

use crate::Provider;
use flikk_core::paths;
use flikk_os::sync::lock;
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const SERVICE: &str = "flikk";

/// After the first failed read of the credential store, before it is asked
/// again; doubled with each failure after it, up to [`RETRY_MAX`].
const RETRY: Duration = Duration::from_secs(15);
const RETRY_MAX: Duration = Duration::from_secs(15 * 60);

/// Where a provider's key comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeySource {
    /// Its environment variable; it can't be changed from the editor.
    Environment,
    Stored,
    None,
}

/// The keys read so far: the credential store is asked once per provider,
/// and again after a while when it couldn't be read.
#[derive(Default)]
pub struct Keys {
    read: Mutex<HashMap<&'static str, Known>>,
    /// Held across each read and write of the stores, so a read can't
    /// cache what a save just replaced.
    stores: Mutex<()>,
}

enum Known {
    Key(Option<String>),
    /// The stores couldn't be read, this many times in a row.
    Failed {
        at: Instant,
        failures: u32,
    },
}

impl Keys {
    /// `provider`'s key. The first call may wait on the credential store
    /// (and on macOS ask the user to allow it), so make it off the UI
    /// thread. While the store can't be read there is none, and the store
    /// is asked again after [`RETRY`] or more.
    pub fn get(&self, provider: &Provider) -> Option<String> {
        if let Some(key) = from_env(provider) {
            return Some(key);
        }
        if let Some(known) = self.known(provider.id) {
            return known;
        }
        let _stores = lock(&self.stores);
        // Read by another thread while this one waited.
        if let Some(known) = self.known(provider.id) {
            return known;
        }
        let failures = match lock(&self.read).get(provider.id) {
            Some(Known::Failed { failures, .. }) => *failures,
            _ => 0,
        };
        let (key, known) = match load(provider.id) {
            Ok(key) => (key.clone(), Known::Key(key)),
            // Not taken for none: asked again in a while.
            Err(_) => (
                None,
                Known::Failed {
                    at: Instant::now(),
                    failures: failures.saturating_add(1),
                },
            ),
        };
        lock(&self.read).insert(provider.id, known);
        key
    }

    /// What is known of `provider`'s key without reading the stores:
    /// `None` when they are to be read.
    fn known(&self, id: &str) -> Option<Option<String>> {
        match lock(&self.read).get(id)? {
            Known::Key(key) => Some(key.clone()),
            Known::Failed { at, failures } => {
                let wait = RETRY
                    .saturating_mul(1 << (failures - 1).min(10))
                    .min(RETRY_MAX);
                (at.elapsed() < wait).then_some(None)
            }
        }
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
        self.save(provider, Some(key))
    }

    pub fn remove(&self, provider: &Provider) -> Result<(), String> {
        self.save(provider, None)
    }

    fn save(&self, provider: &Provider, key: Option<&str>) -> Result<(), String> {
        let _stores = lock(&self.stores);
        let saved = save(provider.id, key);
        let mut read = lock(&self.read);
        match &saved {
            Ok(()) => {
                read.insert(provider.id, Known::Key(key.map(str::to_owned)));
            }
            // Half done, maybe: read again to see what holds.
            Err(_) => {
                read.remove(provider.id);
            }
        }
        saved
    }
}

fn from_env(provider: &Provider) -> Option<String> {
    std::env::var(provider.key_env)
        .ok()
        .map(|k| k.trim().to_owned())
        .filter(|k| !k.is_empty())
}

/// The file's key (none when it is empty), else the credential store's.
/// An error when either couldn't be read: the key may be there.
fn load(id: &str) -> Result<Option<String>, String> {
    if let Some(key) = read_file()?.remove(id) {
        return Ok(Some(key).filter(|k| !k.is_empty()));
    }
    // No credential store on this system, for as long as Flikk runs.
    let Ok(entry) = keyring::Entry::new(SERVICE, id) else {
        return Ok(None);
    };
    match entry.get_password() {
        Ok(key) => Ok(Some(key.trim().to_owned()).filter(|k| !k.is_empty())),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// `key` into the credential store, the file losing any it had; into the
/// file when the store won't take it. `None` is taken out of both, or
/// hidden by an empty key in the file while the store's can't be.
fn save(id: &str, key: Option<&str>) -> Result<(), String> {
    let mut file = read_file()?;
    let before = file.clone();
    let entry = keyring::Entry::new(SERVICE, id);
    let in_store = match (&entry, key) {
        (Ok(entry), Some(key)) => entry.set_password(key).is_ok(),
        (Ok(entry), None) => matches!(
            entry.delete_credential(),
            Ok(()) | Err(keyring::Error::NoEntry)
        ),
        (Err(_), _) => false,
    };
    if in_store {
        file.remove(id);
    } else {
        file.insert(id.to_owned(), key.unwrap_or_default().to_owned());
    }
    if file != before {
        write_file(&file)?;
    }
    Ok(())
}

/// The file's keys: none when there is no file, or it is garbled (it is
/// written over).
fn read_file() -> Result<BTreeMap<String, String>, String> {
    let path = paths::credentials();
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(serde_json::from_str(&text).unwrap_or_default()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(format!("could not read {}: {e}", path.display())),
    }
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
