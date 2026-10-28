//! Downloads the user starts: a model's missing files fetched one after
//! another on a thread of its own, each through a temporary file so a cut
//! download never looks complete.

use crate::{Model, ModelFile};
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// Where a model stands.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// Every file is on disk.
    Downloaded,
    /// A file is not, and nothing is fetching it.
    Missing,
    /// Fetching; 0..1 of its bytes.
    Downloading(f32),
    /// The last download failed, and why.
    Failed(String),
}

/// The downloads the user started. Clones share them.
#[derive(Clone, Default)]
pub struct Downloads {
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    /// By model id. A download that ends leaves only a failure behind.
    jobs: Mutex<HashMap<&'static str, Job>>,
    /// Downloads ended so far, however they ended.
    finished: AtomicU64,
}

enum Job {
    Running {
        fraction: f32,
        cancel: Arc<AtomicBool>,
    },
    Failed(String),
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Downloads {
    pub fn status(&self, model: &Model) -> Status {
        let failed = match lock(&self.shared.jobs).get(model.id) {
            Some(Job::Running { fraction, .. }) => return Status::Downloading(*fraction),
            Some(Job::Failed(message)) => Some(message.clone()),
            None => None,
        };
        match failed {
            _ if model.is_downloaded() => Status::Downloaded,
            Some(message) => Status::Failed(message),
            None => Status::Missing,
        }
    }

    /// Fetch `model`'s missing files on a thread of their own. Does nothing
    /// while it is already downloading.
    pub fn start(&self, model: &'static Model) {
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut jobs = lock(&self.shared.jobs);
            if matches!(jobs.get(model.id), Some(Job::Running { .. })) {
                return;
            }
            jobs.insert(
                model.id,
                Job::Running {
                    fraction: 0.0,
                    cancel: cancel.clone(),
                },
            );
        }
        let shared = self.shared.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("koo-models-{}", model.id))
            .spawn(move || {
                let result = fetch_model(model, &cancel, &|f| shared.progress(model.id, f));
                let failed = result
                    .err()
                    .filter(|_| !cancel.load(Ordering::Relaxed))
                    .map(|e| format!("{e:#}"));
                shared.end(model.id, failed);
            });
        if let Err(e) = spawned {
            self.shared
                .end(model.id, Some(format!("could not start the download: {e}")));
        }
    }

    /// Stop `model`'s download; what it fetched so far is thrown away.
    pub fn cancel(&self, model: &Model) {
        if let Some(Job::Running { cancel, .. }) = lock(&self.shared.jobs).get(model.id) {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Whether any download is running.
    pub fn busy(&self) -> bool {
        lock(&self.shared.jobs)
            .values()
            .any(|j| matches!(j, Job::Running { .. }))
    }

    /// Changes each time a download ends, done, failed or cancelled.
    pub fn finished(&self) -> u64 {
        self.shared.finished.load(Ordering::Acquire)
    }
}

impl Shared {
    fn progress(&self, id: &str, f: f32) {
        if let Some(Job::Running { fraction, .. }) = lock(&self.jobs).get_mut(id) {
            *fraction = f.clamp(0.0, 1.0);
        }
    }

    fn end(&self, id: &'static str, failed: Option<String>) {
        let mut jobs = lock(&self.jobs);
        match failed {
            Some(message) => jobs.insert(id, Job::Failed(message)),
            None => jobs.remove(id),
        };
        drop(jobs);
        self.finished.fetch_add(1, Ordering::Release);
    }
}

/// Fetch each of `model`'s files not on disk, reporting 0..1 of their
/// bytes.
fn fetch_model(model: &Model, cancel: &AtomicBool, progress: &dyn Fn(f32)) -> Result<()> {
    let missing: Vec<&ModelFile> = model.files.iter().filter(|f| !f.is_present()).collect();
    let total = missing.iter().map(|f| f.size).sum::<u64>().max(1) as f32;
    let mut done = 0;
    for file in missing {
        // A copy the environment points at is the user's to provide.
        if file.overridden().is_some() {
            file.locate()?;
            continue;
        }
        fetch(file, &file.path(), cancel, &|bytes| {
            progress((done + bytes) as f32 / total)
        })?;
        done += file.size;
    }
    Ok(())
}

/// Fetch `file` into `dest` via `<dest>.part`, reporting the bytes read.
fn fetch(file: &ModelFile, dest: &Path, cancel: &AtomicBool, progress: &dyn Fn(u64)) -> Result<()> {
    let mut part = dest.as_os_str().to_owned();
    part.push(".part");
    let part = Path::new(&part);
    let result = fetch_into(file, part, cancel, progress)
        .and_then(|()| std::fs::rename(part, dest).map_err(Into::into));
    if result.is_err() {
        let _ = std::fs::remove_file(part);
    }
    result
}

fn fetch_into(
    file: &ModelFile,
    part: &Path,
    cancel: &AtomicBool,
    progress: &dyn Fn(u64),
) -> Result<()> {
    use ureq::tls::{RootCerts, TlsConfig};
    let what = file.what;
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
        .get(file.url)
        .call()
        .with_context(|| format!("could not download {what}"))?;
    let total = response.body().content_length();
    let mut body = response.body_mut().as_reader();

    let mut out = std::fs::File::create(part)
        .with_context(|| format!("could not write {}", part.display()))?;
    let mut buf = vec![0u8; 1 << 20];
    let mut done = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            bail!("{what} download cancelled");
        }
        let n = body
            .read(&mut buf)
            .with_context(|| format!("{what} download interrupted"))?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])
            .with_context(|| format!("could not write {}", part.display()))?;
        done += n as u64;
        progress(done);
    }
    out.sync_all()?;
    if total.is_some_and(|t| t != done) {
        bail!("{what} download incomplete ({done} bytes)");
    }
    Ok(())
}
