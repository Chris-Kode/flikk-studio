//! Cached frames on disk, under `paths::cache()/frame-cache/`: one QOI file per
//! [`Key`] (in a folder per first byte), written atomically, the least
//! recently used removed past the size limit. The index is read from the
//! folder on first use, ordered by the files' modified times, which reads
//! touch.

use super::key::Key;
use flikk_core::paths;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

/// Starts every file, before its key and the QOI picture.
const MAGIC: &[u8; 4] = b"KOF1";
const EXTENSION: &str = "qoi";
const GB: u64 = 1_000_000_000;

/// A frame read back: BGRA.
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

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

pub struct Store {
    dir: PathBuf,
    index: Mutex<Index>,
    /// Background rendering is on.
    background: AtomicBool,
    limit: AtomicU64,
    /// Counts frames written and removed, so views know to look again.
    version: AtomicU64,
}

/// The one store, its index read on first use.
pub fn store() -> &'static Store {
    static STORE: OnceLock<Store> = OnceLock::new();
    STORE.get_or_init(|| {
        let settings = flikk_store::RenderCacheSettings::default();
        let store = Store {
            dir: dir(),
            index: Mutex::new(Index::default()),
            background: AtomicBool::new(settings.background),
            limit: AtomicU64::new(settings.limit_gb as u64 * GB),
            version: AtomicU64::new(0),
        };
        store.scan();
        store
    })
}

/// Where cached frames live.
pub fn dir() -> PathBuf {
    paths::cache().join("frame-cache")
}

impl Store {
    pub fn background(&self) -> bool {
        self.background.load(Ordering::Relaxed)
    }

    pub fn apply(&self, settings: &flikk_store::RenderCacheSettings) {
        self.background
            .store(settings.background, Ordering::Relaxed);
        self.limit
            .store(settings.limit_gb.max(1) as u64 * GB, Ordering::Relaxed);
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

    /// The frame cached as `key`. A file that went missing or does not
    /// read is forgotten, so the frame is composed instead.
    pub fn read(&self, key: &Key) -> Option<Picture> {
        if !self.contains(key) {
            return None;
        }
        let path = self.path(key);
        let picture = fs::read(&path).ok().and_then(|bytes| decode(key, &bytes));
        let mut index = self.lock();
        match picture {
            Some(p) => {
                index.touch(*key);
                drop(index);
                if let Ok(file) = fs::File::options().write(true).open(&path) {
                    let _ = file.set_modified(SystemTime::now());
                }
                Some(p)
            }
            None => {
                if index.remove(*key) {
                    self.bump();
                }
                drop(index);
                let _ = fs::remove_file(path);
                None
            }
        }
    }

    /// Keep a `width` × `height` BGRA frame as `key`.
    pub fn write(&self, key: &Key, width: u32, height: u32, bgra: &[u8]) {
        let Ok(qoi) = qoi::encode_to_vec(bgra, width, height) else {
            return;
        };
        let path = self.path(key);
        let Some(folder) = path.parent() else {
            return;
        };
        let _ = fs::create_dir_all(folder);
        let tmp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4().simple()));
        let written = fs::File::create(&tmp).and_then(|mut f| {
            f.write_all(MAGIC)?;
            f.write_all(key)?;
            f.write_all(&qoi)
        });
        if written.is_err() || fs::rename(&tmp, &path).is_err() {
            let _ = fs::remove_file(&tmp);
            return;
        }
        let bytes = (MAGIC.len() + key.len() + qoi.len()) as u64;
        let mut index = self.lock();
        index.insert(*key, bytes);
        self.evict(&mut index);
        self.bump();
    }

    /// Remove every cached frame.
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
        self.dir.join(&hex[..2]).join(format!("{hex}.{EXTENSION}"))
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
                    .filter(|_| ext == EXTENSION)
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

fn decode(key: &Key, bytes: &[u8]) -> Option<Picture> {
    let rest = bytes.strip_prefix(MAGIC.as_slice())?;
    let rest = rest.strip_prefix(key.as_slice())?;
    let (header, bgra) = qoi::decode_to_vec(rest).ok()?;
    (header.channels == qoi::Channels::Rgba).then_some(Picture {
        width: header.width,
        height: header.height,
        bgra,
    })
}
