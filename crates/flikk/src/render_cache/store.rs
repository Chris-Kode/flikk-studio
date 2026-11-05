//! Cached frames on disk, under `paths::cache()/frame-cache/<generation>/`
//! ([`DiskCache`]), the least recently used removed past the size limit.
//! The generation names the layout and the app version, so frames no
//! later version could ask for go at once instead of filling the limit.
//!
//! Frames are only ever shown in the viewer (renders compose their own),
//! so opaque ones, nearly all, are kept as JPEG, about a tenth the size of
//! lossless; one with any transparency is kept as QOI.

use super::key::{FORMAT, Key};
use flikk_core::disk_cache::{DiskCache, GB};
use flikk_core::paths;
use image::ImageEncoder as _;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

/// Starts every file, before its key and the picture.
const MAGIC: [u8; 4] = *b"KOF2";
const EXTENSION: &str = "frame";
/// Nearly indistinguishable from the composed frame at viewer sizes.
const JPEG_QUALITY: u8 = 90;

/// A frame read back: BGRA.
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

pub struct Store {
    files: DiskCache,
    /// Background rendering is on.
    background: AtomicBool,
}

/// The one store, its index read on first use.
pub fn store() -> &'static Store {
    static STORE: OnceLock<Store> = OnceLock::new();
    STORE.get_or_init(|| {
        let settings = flikk_store::RenderCacheSettings::default();
        let generation = format!("v{FORMAT}-{}", env!("CARGO_PKG_VERSION"));
        Store {
            files: DiskCache::open(&dir(), &generation, MAGIC, EXTENSION, limit(&settings)),
            background: AtomicBool::new(settings.background),
        }
    })
}

/// Where cached frames live.
fn dir() -> PathBuf {
    paths::cache().join("frame-cache")
}

fn limit(settings: &flikk_store::RenderCacheSettings) -> u64 {
    settings.limit_gb.max(1) as u64 * GB
}

impl Store {
    pub fn background(&self) -> bool {
        self.background.load(Ordering::Relaxed)
    }

    pub fn apply(&self, settings: &flikk_store::RenderCacheSettings) {
        self.background
            .store(settings.background, Ordering::Relaxed);
        self.files.set_limit(limit(settings));
    }

    pub fn version(&self) -> u64 {
        self.files.version()
    }

    pub fn contains(&self, key: &Key) -> bool {
        self.files.contains(key)
    }

    /// The frame cached as `key`. One that does not read is forgotten, so
    /// the frame is composed instead.
    pub fn read(&self, key: &Key) -> Option<Picture> {
        let bytes = self.files.read(key)?;
        let picture = decode(&bytes);
        if picture.is_none() {
            self.files.forget(key);
        }
        picture
    }

    /// Keep a `width` × `height` BGRA frame as `key`.
    pub fn write(&self, key: &Key, width: u32, height: u32, bgra: &[u8]) {
        if let Some(bytes) = encode(width, height, bgra) {
            self.files.write(key, &bytes);
        }
    }

    /// Remove every cached frame.
    pub fn clear(&self) {
        self.files.clear();
    }
}

fn encode(width: u32, height: u32, bgra: &[u8]) -> Option<Vec<u8>> {
    if bgra.len() != width as usize * height as usize * 4 {
        return None;
    }
    let (pixels, _) = bgra.as_chunks::<4>();
    if pixels.iter().any(|p| p[3] != 255) {
        return qoi::encode_to_vec(bgra, width, height).ok();
    }
    let rgb: Vec<u8> = pixels.iter().flat_map(|p| [p[2], p[1], p[0]]).collect();
    let mut out = Vec::with_capacity(rgb.len() / 8);
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY)
        .write_image(&rgb, width, height, image::ExtendedColorType::Rgb8)
        .ok()?;
    Some(out)
}

fn decode(bytes: &[u8]) -> Option<Picture> {
    if bytes.starts_with(b"qoif") {
        let (header, bgra) = qoi::decode_to_vec(bytes).ok()?;
        return (header.channels == qoi::Channels::Rgba).then_some(Picture {
            width: header.width,
            height: header.height,
            bgra,
        });
    }
    let rgb = image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg)
        .ok()?
        .into_rgb8();
    let (width, height) = rgb.dimensions();
    let bgra = rgb
        .as_raw()
        .as_chunks::<3>()
        .0
        .iter()
        .flat_map(|p| [p[2], p[1], p[0], 255])
        .collect();
    Some(Picture {
        width,
        height,
        bgra,
    })
}
