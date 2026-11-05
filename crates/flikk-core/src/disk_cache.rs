//! Files made again when missing, kept on disk under a size limit: one
//! file per 16-byte [`Key`] (in a folder per first byte), each starting
//! with the cache's magic and its key, written atomically, the least
//! recently used removed past the limit.
//!
//! A cache lives in `<root>/<generation>/`; opening it removes the other
//! generations, so a new layout or app version does not leave the old
//! files to age out. The index is read from the folder on first use,
//! ordered by the files' modified times, which reads touch.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

/// A file's name.
pub type Key = [u8; 16];

pub const GB: u64 = 1_000_000_000;

#[derive(Default)]
struct Index {
    files: HashMap<Key, (u64, u64)>,
    /// Use ticks to keys, oldest first.
    order: BTreeMap<u64, Key>,
    tick: u64,
    bytes: u64,
}

impl Index {
    fn touch(&mut self, key: Key) {
        self.tick += 1;
        let tick = self.tick;
        if let Some((_, t)) = self.files.get_mut(&key) {
            self.order.remove(t);
            *t = tick;
            self.order.insert(tick, key);
        }
    }

    fn insert(&mut self, key: Key, bytes: u64) {
        self.remove(key);
        self.tick += 1;
        self.files.insert(key, (bytes, self.tick));
        self.order.insert(self.tick, key);
        self.bytes += bytes;
    }

    fn remove(&mut self, key: Key) -> bool {
        match self.files.remove(&key) {
            Some((bytes, tick)) => {
                self.order.remove(&tick);
                self.bytes -= bytes;
                true
            }
            None => false,
        }
    }
}

pub struct DiskCache {
    dir: PathBuf,
    magic: [u8; 4],
    extension: &'static str,
    index: Mutex<Index>,
    limit: AtomicU64,
    /// Counts files written and removed, so views know to look again.
    version: AtomicU64,
}

impl DiskCache {
    /// The cache in `root/generation`, its index read from the folder;
    /// other generations under `root` are removed.
    pub fn open(
        root: &Path,
        generation: &str,
        magic: [u8; 4],
        extension: &'static str,
        limit: u64,
    ) -> Self {
        for old in read_dir(root) {
            if old.file_name().is_some_and(|n| n != generation) {
                let _ = if old.is_dir() {
                    fs::remove_dir_all(&old)
                } else {
                    fs::remove_file(&old)
                };
            }
        }
        let cache = Self {
            dir: root.join(generation),
            magic,
            extension,
            index: Mutex::new(Index::default()),
            limit: AtomicU64::new(limit),
            version: AtomicU64::new(0),
        };
        cache.scan();
        cache
    }

    pub fn set_limit(&self, limit: u64) {
        self.limit.store(limit, Ordering::Relaxed);
        let mut index = self.lock();
        self.evict(&mut index);
    }

    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Relaxed)
    }

    pub fn bytes(&self) -> u64 {
        self.lock().bytes
    }

    pub fn contains(&self, key: &Key) -> bool {
        self.lock().files.contains_key(key)
    }

    /// What is kept as `key`, without the magic and key it starts with.
    /// A file that went missing or is not this cache's is forgotten.
    pub fn read(&self, key: &Key) -> Option<Vec<u8>> {
        if !self.contains(key) {
            return None;
        }
        let path = self.path(key);
        let body = fs::read(&path).ok().and_then(|mut bytes| {
            let head = self.magic.len() + key.len();
            (bytes.len() >= head && bytes[..4] == self.magic && bytes[4..head] == key[..]).then(
                || {
                    bytes.drain(..head);
                    bytes
                },
            )
        });
        match body {
            Some(body) => {
                self.lock().touch(*key);
                if let Ok(file) = fs::File::options().write(true).open(&path) {
                    let _ = file.set_modified(SystemTime::now());
                }
                Some(body)
            }
            None => {
                self.forget(key);
                None
            }
        }
    }

    /// Drop `key`, for one that read but would not decode.
    pub fn forget(&self, key: &Key) {
        let mut index = self.lock();
        if index.remove(*key) {
            self.bump();
        }
        drop(index);
        let _ = fs::remove_file(self.path(key));
    }

    /// Keep `body` as `key`.
    pub fn write(&self, key: &Key, body: &[u8]) {
        let path = self.path(key);
        let Some(folder) = path.parent() else {
            return;
        };
        let _ = fs::create_dir_all(folder);
        let tmp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4().simple()));
        let written = fs::File::create(&tmp).and_then(|mut f| {
            f.write_all(&self.magic)?;
            f.write_all(key)?;
            f.write_all(body)
        });
        if written.is_err() || fs::rename(&tmp, &path).is_err() {
            let _ = fs::remove_file(&tmp);
            return;
        }
        let bytes = (self.magic.len() + key.len() + body.len()) as u64;
        let mut index = self.lock();
        index.insert(*key, bytes);
        self.evict(&mut index);
        self.bump();
    }

    /// Remove every file.
    pub fn clear(&self) {
        let mut index = self.lock();
        *index = Index::default();
        let _ = fs::remove_dir_all(&self.dir);
        self.bump();
    }

    fn evict(&self, index: &mut Index) {
        let limit = self.limit.load(Ordering::Relaxed);
        let mut evicted = false;
        while index.bytes > limit
            && let Some((_, key)) = index.order.pop_first()
        {
            index.remove(key);
            let _ = fs::remove_file(self.path(&key));
            evicted = true;
        }
        if evicted {
            self.bump();
        }
    }

    fn bump(&self) {
        self.version.fetch_add(1, Ordering::Relaxed);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Index> {
        self.index.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn path(&self, key: &Key) -> PathBuf {
        let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
        self.dir
            .join(&hex[..2])
            .join(format!("{hex}.{}", self.extension))
    }

    /// Index what the folder holds, oldest use first; leftovers of
    /// interrupted writes go.
    fn scan(&self) {
        let mut found: Vec<(SystemTime, Key, u64)> = Vec::new();
        for folder in read_dir(&self.dir) {
            for file in read_dir(&folder) {
                let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("");
                if ext.starts_with("tmp-") {
                    let _ = fs::remove_file(&file);
                    continue;
                }
                let key = file
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .filter(|_| ext == self.extension)
                    .and_then(parse_key);
                let Some(key) = key else {
                    continue;
                };
                let Ok(meta) = file.metadata() else {
                    continue;
                };
                let used = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                found.push((used, key, meta.len()));
            }
        }
        found.sort_by_key(|(used, ..)| *used);
        let mut index = self.lock();
        for (_, key, bytes) in found {
            index.insert(key, bytes);
        }
        self.evict(&mut index);
    }
}

fn read_dir(dir: &Path) -> impl Iterator<Item = PathBuf> {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
}

fn parse_key(hex: &str) -> Option<Key> {
    if hex.len() != 32 {
        return None;
    }
    let mut key = [0; 16];
    for (i, b) in key.iter_mut().enumerate() {
        *b = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(key)
}
