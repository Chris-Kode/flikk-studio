//! Downloads the user starts: a model's missing files fetched one after
//! another on a thread of its own, each through a temporary file so a cut
//! download never looks complete. A server that stops sending fails the
//! download after a minute.

use crate::{Model, ModelFile};
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// How long the server may send nothing, before the response and between
/// body reads, before the download fails. A whole file may take as long as
/// it takes.
const STALL: Duration = Duration::from_secs(60);

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
    /// Numbers each download, so a cancelled one still winding down
    /// leaves the next one's state and files alone.
    next_job: AtomicU64,
}

enum Job {
    Running {
        job: u64,
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
        let job = self.shared.next_job.fetch_add(1, Ordering::Relaxed);
        {
            let mut jobs = lock(&self.shared.jobs);
            if matches!(jobs.get(model.id), Some(Job::Running { .. })) {
                return;
            }
            jobs.insert(
                model.id,
                Job::Running {
                    job,
                    fraction: 0.0,
                    cancel: cancel.clone(),
                },
            );
        }
        let shared = self.shared.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("flikk-models-{}", model.id))
            .spawn(move || {
                let result =
                    fetch_model(model, job, &cancel, &|f| shared.progress(model.id, job, f));
                shared.end(model.id, job, result.err().map(|e| format!("{e:#}")));
            });
        if let Err(e) = spawned {
            self.shared.end(
                model.id,
                job,
                Some(format!("could not start the download: {e}")),
            );
        }
    }

    /// Stop `model`'s download at once: the file it is fetching is thrown
    /// away, the files it finished stay. It can be started again right
    /// away.
    pub fn cancel(&self, model: &Model) {
        let mut jobs = lock(&self.shared.jobs);
        if let Some(Job::Running { cancel, .. }) = jobs.get(model.id) {
            // Its thread may be stuck in a read; it ends on its own, and
            // finds itself no longer the job.
            cancel.store(true, Ordering::Relaxed);
            jobs.remove(model.id);
            drop(jobs);
            self.shared.finished.fetch_add(1, Ordering::Release);
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
    fn progress(&self, id: &str, job: u64, f: f32) {
        if let Some(Job::Running {
            job: j, fraction, ..
        }) = lock(&self.jobs).get_mut(id)
            && *j == job
        {
            *fraction = f.clamp(0.0, 1.0);
        }
    }

    /// `job` ended; nothing changes unless it is still `id`'s download.
    fn end(&self, id: &'static str, job: u64, failed: Option<String>) {
        let mut jobs = lock(&self.jobs);
        if !matches!(jobs.get(id), Some(Job::Running { job: j, .. }) if *j == job) {
            return;
        }
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
fn fetch_model(model: &Model, job: u64, cancel: &AtomicBool, progress: &dyn Fn(f32)) -> Result<()> {
    let missing: Vec<&ModelFile> = model.files.iter().filter(|f| !f.is_present()).collect();
    let total = missing.iter().map(|f| f.size).sum::<u64>().max(1) as f32;
    let mut done = 0;
    for file in missing {
        // A copy the environment points at is the user's to provide.
        if file.overridden().is_some() {
            file.locate()?;
            continue;
        }
        if cancel.load(Ordering::Relaxed) {
            bail!("{} download cancelled", file.what);
        }
        fetch(file, &file.path(), job, cancel, &|bytes| {
            progress((done + bytes) as f32 / total)
        })?;
        done += file.size;
    }
    Ok(())
}

/// Fetch `file` into `dest` via `<dest>.<process>-<job>.part`, its own
/// so a cancelled download still winding down never shares it, reporting
/// the bytes read. Parts other runs left behind go first.
fn fetch(
    file: &ModelFile,
    dest: &Path,
    job: u64,
    cancel: &AtomicBool,
    progress: &dyn Fn(u64),
) -> Result<()> {
    remove_stale_parts(dest);
    let mut part = dest.as_os_str().to_owned();
    part.push(format!(".{}-{job}.part", std::process::id()));
    let part = Path::new(&part);
    let result = fetch_into(file, part, cancel, progress).and_then(|()| {
        if cancel.load(Ordering::Relaxed) {
            bail!("{} download cancelled", file.what);
        }
        std::fs::rename(part, dest).map_err(Into::into)
    });
    if result.is_err() {
        let _ = std::fs::remove_file(part);
    }
    result
}

/// Delete `dest`'s parts from runs of the app that ended: `<dest>.part`,
/// or `<dest>.<process>-<job>.part` of another process. This one's own
/// downloads delete theirs.
fn remove_stale_parts(dest: &Path) {
    let (Some(dir), Some(name)) = (dest.parent(), dest.file_name().and_then(|n| n.to_str())) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let ours = format!("{}-", std::process::id());
    for entry in entries.flatten() {
        let stale = entry.file_name().to_str().is_some_and(|n| {
            let Some(tag) = n.strip_prefix(name).and_then(|t| t.strip_suffix(".part")) else {
                return false;
            };
            let Some(tag) = tag.strip_prefix('.') else {
                return tag.is_empty();
            };
            let ids = tag.split_once('-').filter(|(p, j)| {
                [p, j]
                    .iter()
                    .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            });
            ids.is_some() && !tag.starts_with(&ours)
        });
        if stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Whether `e`, or what caused it, is ureq or the socket giving up waiting.
fn timed_out(e: &(dyn std::error::Error + 'static)) -> bool {
    if let Some(e) = e.downcast_ref::<ureq::Error>() {
        return match e {
            ureq::Error::Timeout(_) => true,
            ureq::Error::Io(e) => timed_out(e),
            _ => false,
        };
    }
    if let Some(e) = e.downcast_ref::<std::io::Error>() {
        // What it wraps is not its `source`.
        return e.kind() == std::io::ErrorKind::TimedOut
            || e.get_ref().is_some_and(|e| timed_out(e));
    }
    e.source().is_some_and(timed_out)
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
        .timeout_send_request(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(STALL))
        .timeout_per_read(Some(STALL))
        // No limit on the whole body: a large model on a slow link is fine
        // as long as bytes keep coming.
        .timeout_recv_body(None)
        .build()
        .into();
    let stalled = || {
        format!(
            "{what} download stalled: the server sent nothing for {} s",
            STALL.as_secs()
        )
    };
    let mut response = match agent.get(file.url).call() {
        Ok(response) => response,
        Err(e) if timed_out(&e) => bail!(stalled()),
        Err(e) => return Err(e).with_context(|| format!("could not download {what}")),
    };
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
        let n = match body.read(&mut buf) {
            Ok(n) => n,
            Err(e) if timed_out(&e) => bail!(stalled()),
            Err(e) => return Err(e).with_context(|| format!("{what} download interrupted")),
        };
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
