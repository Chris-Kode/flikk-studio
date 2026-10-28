//! Model files fetched into `paths::models()` on first use.

use anyhow::{Context, Result, bail};
use koo_core::paths;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Starts a download's job, given the file's name, and reports its
/// progress, 0..1.
#[cfg_attr(not(any(feature = "whisper", feature = "sounds")), allow(dead_code))]
pub type StartDownload<'a> = &'a dyn Fn(&str) -> Box<dyn Fn(f32) + Send + Sync>;

/// One model file: its name in `paths::models()`, where it downloads from
/// and the environment variable that points at a copy instead.
#[cfg_attr(not(any(feature = "whisper", feature = "sounds")), allow(dead_code))]
pub struct ModelFile {
    pub name: &'static str,
    pub url: &'static str,
    /// A path, never fetched.
    pub env: &'static str,
    /// What the user reads in errors and the download job: "sound tagger".
    pub what: &'static str,
}

#[cfg_attr(not(any(feature = "whisper", feature = "sounds")), allow(dead_code))]
impl ModelFile {
    /// The file on disk, downloading it first if needed.
    pub fn ensure(&self, download: &Download, start: StartDownload) -> Result<PathBuf> {
        if let Some(path) = std::env::var_os(self.env).filter(|v| !v.is_empty()) {
            let path = PathBuf::from(path);
            if !path.is_file() {
                bail!(
                    "{} not found at {} (from {})",
                    self.what,
                    path.display(),
                    self.env
                );
            }
            return Ok(path);
        }
        let path = paths::models().join(self.name);
        download.ensure(&path, self.url, self.what, || start(self.name))?;
        Ok(path)
    }
}

/// How long a failed download is reported before trying again.
const RETRY_AFTER: Duration = Duration::from_secs(120);

/// One model file, downloaded by one caller at a time.
#[derive(Default)]
pub struct Download {
    /// The last failure, so a queue of items doesn't retry it each.
    failed: Mutex<Option<(Instant, String)>>,
}

impl Download {
    /// Make sure `dest` exists, fetching `what` from `url` if it does not;
    /// `start` returns a progress callback for the download.
    pub fn ensure(
        &self,
        dest: &Path,
        url: &str,
        what: &str,
        start: impl FnOnce() -> Box<dyn Fn(f32) + Send + Sync>,
    ) -> Result<()> {
        if dest.is_file() {
            return Ok(());
        }
        // Later callers find the file in place.
        let mut failed = self.failed.lock().unwrap_or_else(|e| e.into_inner());
        if dest.is_file() {
            return Ok(());
        }
        if let Some((at, message)) = &*failed
            && at.elapsed() < RETRY_AFTER
        {
            bail!("{message}");
        }
        match fetch(url, dest, what, &*start()) {
            Ok(()) => Ok(()),
            Err(e) => {
                *failed = Some((Instant::now(), format!("{e:#}")));
                Err(e)
            }
        }
    }
}

/// Fetch `url` into `dest` via a temporary file, so a cut download never
/// looks complete.
fn fetch(url: &str, dest: &Path, what: &str, progress: &(dyn Fn(f32) + Send + Sync)) -> Result<()> {
    use ureq::tls::{RootCerts, TlsConfig};
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(
            TlsConfig::builder()
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_body(None)
        .build()
        .into();
    let mut response = agent
        .get(url)
        .call()
        .with_context(|| format!("could not download {what} from {url}"))?;
    let total = response.body().content_length();
    let mut body = response.body_mut().as_reader();

    let mut part = dest.as_os_str().to_owned();
    part.push(".part");
    let part = Path::new(&part);
    let mut file = std::fs::File::create(part)
        .with_context(|| format!("could not write {}", part.display()))?;
    let mut buf = vec![0u8; 1 << 20];
    let mut done = 0u64;
    loop {
        let n = body
            .read(&mut buf)
            .with_context(|| format!("{what} download interrupted"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        done += n as u64;
        if let Some(total) = total.filter(|t| *t > 0) {
            progress(done as f32 / total as f32);
        }
    }
    file.sync_all()?;
    drop(file);
    if total.is_some_and(|t| t != done) {
        let _ = std::fs::remove_file(part);
        bail!("{what} download incomplete ({done} bytes)");
    }
    std::fs::rename(part, dest)?;
    Ok(())
}
