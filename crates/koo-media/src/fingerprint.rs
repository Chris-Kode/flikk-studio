//! BLAKE3 over the file size, head, tail and 8 evenly spaced 1 MiB chunks:
//! fast on multi-GB files and enough to notice a replaced source (PRD F2).

use crate::err;
use koo_core::model::Fingerprint;
use koo_core::services::ServiceResult;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const CHUNK: u64 = 1 << 20;
const MIDDLE_CHUNKS: u64 = 8;

pub(crate) fn fingerprint(path: &Path) -> ServiceResult<Fingerprint> {
    let context = |e| err(&format!("fingerprint {}", path.display()), e);
    let mut file = File::open(path).map_err(context)?;
    let size = file.metadata().map_err(context)?.len();

    let mut hasher = blake3::Hasher::new();
    hasher.update(&size.to_le_bytes());

    // Small files: the sampled chunks would cover most of it anyway.
    if size <= CHUNK * (MIDDLE_CHUNKS + 2) {
        std::io::copy(&mut file, &mut hasher).map_err(context)?;
        return Ok(Fingerprint(hasher.finalize().to_hex().to_string()));
    }

    let mut offsets = vec![0];
    let span = size - CHUNK;
    offsets.extend((1..=MIDDLE_CHUNKS).map(|i| span * i / (MIDDLE_CHUNKS + 1)));
    offsets.push(span);

    let mut buf = vec![0u8; CHUNK as usize];
    for offset in offsets {
        file.seek(SeekFrom::Start(offset)).map_err(context)?;
        file.read_exact(&mut buf).map_err(context)?;
        hasher.update(&offset.to_le_bytes());
        hasher.update(&buf);
    }
    Ok(Fingerprint(hasher.finalize().to_hex().to_string()))
}
