//! Downloads the user starts: a model's missing files fetched one after
//! another on a thread of its own, each through `<file>.part` so a cut
//! download never looks complete, and a cancelled or cut one goes on from
//! where it stopped. A server that stops sending fails the download after
//! a minute.

use crate::{Model, ModelFile};
use anyhow::{Context, Result, bail};
use flikk_os::sync::lock;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long the server may send nothing, before the response and between
/// body reads, before the download fails. A whole file may take as long as
/// it takes.
const STALL: Duration = Duration::from_secs(60);

/// Where a model stands.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// Every file is on disk and whole.
    Downloaded,
    /// A file is not (or is cut short or damaged), and nothing is fetching
    /// it.
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
    /// leaves the next one's state alone.
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
                let result = fetch_model(model, &cancel, &|f| shared.progress(model.id, job, f));
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

    /// Stop `model`'s download at once: the files it finished stay, and
    /// the one it is fetching goes on from where it stopped next time. It
    /// can be started again right away.
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
/// bytes. A file part-fetched before counts as done that far.
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
        if cancel.load(Ordering::Relaxed) {
            bail!("{} download cancelled", file.what);
        }
        fetch(file, &file.path(), cancel, &|bytes| {
            progress((done + bytes) as f32 / total)
        })?;
        done += file.size;
    }
    Ok(())
}

/// How often a download waiting for its part file looks again.
const PART_WAIT: Duration = Duration::from_millis(100);

/// Fetch `file` into `dest` through `<dest>.part`, reporting the bytes on
/// disk. What a part already holds (from a cancelled or cut download) is
/// kept and the rest asked for. The part is locked while it is written: a
/// cancelled download still winding down, or another copy of the app,
/// finishes with it first. A part that turns out damaged is deleted; any
/// other failure leaves it for the next try.
fn fetch(file: &ModelFile, dest: &Path, cancel: &AtomicBool, progress: &dyn Fn(u64)) -> Result<()> {
    remove_stale_parts(dest);
    let part = part_path(dest);
    let mut out = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&part)
        .with_context(|| format!("could not write {}", part.display()))?;
    loop {
        match out.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) => {
                if cancel.load(Ordering::Relaxed) {
                    bail!("{} download cancelled", file.what);
                }
                std::thread::sleep(PART_WAIT);
            }
            Err(std::fs::TryLockError::Error(e)) => {
                return Err(e).with_context(|| format!("could not lock {}", part.display()));
            }
        }
    }
    let result = fetch_into(file, &mut out, cancel, progress);
    // Unlocked before it is renamed or removed: Windows refuses either on
    // a locked file.
    drop(out);
    match result {
        Ok(()) if cancel.load(Ordering::Relaxed) => bail!("{} download cancelled", file.what),
        Ok(()) => match std::fs::rename(&part, dest) {
            // Another copy of the app renamed the same whole part first.
            Err(_) if file.is_present() => Ok(()),
            renamed => renamed.with_context(|| format!("could not write {}", dest.display())),
        },
        Err(Fetch::Damaged(e)) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
        Err(Fetch::Kept(e)) => Err(e),
    }
}

/// Where `dest` is fetched into: `<dest>.part`.
pub(crate) fn part_path(dest: &Path) -> std::path::PathBuf {
    let mut part = dest.as_os_str().to_owned();
    part.push(".part");
    part.into()
}

/// Delete the parts older builds left for `dest`, named after their
/// process and job (`<dest>.<process>-<job>.part`): they could not be
/// resumed.
fn remove_stale_parts(dest: &Path) {
    let (Some(dir), Some(name)) = (dest.parent(), dest.file_name().and_then(|n| n.to_str())) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let stale = entry.file_name().to_str().is_some_and(|n| {
            let Some(tag) = n
                .strip_prefix(name)
                .and_then(|t| t.strip_suffix(".part"))
                .and_then(|t| t.strip_prefix('.'))
            else {
                return false;
            };
            tag.split_once('-').is_some_and(|(p, j)| {
                [p, j]
                    .iter()
                    .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            })
        });
        if stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Why [`fetch_into`] failed, and whether its part is worth keeping.
enum Fetch {
    /// Cut short or cancelled: what the part holds is good.
    Kept(anyhow::Error),
    /// The part holds the wrong bytes.
    Damaged(anyhow::Error),
}

impl<E: Into<anyhow::Error>> From<E> for Fetch {
    fn from(e: E) -> Self {
        Fetch::Kept(e.into())
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

/// Fill `out`, the locked part, with `file`: from where it ends when the
/// server can send the rest, else from the start.
fn fetch_into(
    file: &ModelFile,
    out: &mut std::fs::File,
    cancel: &AtomicBool,
    progress: &dyn Fn(u64),
) -> std::result::Result<(), Fetch> {
    use std::io::{Seek, SeekFrom};
    use ureq::tls::{RootCerts, TlsConfig};
    let what = file.what;
    let mut done = out.metadata()?.len();
    if file.size != 0 && done > file.size {
        out.set_len(0)?;
        done = 0;
    }
    if file.size != 0 && done == file.size {
        progress(done);
        return Ok(());
    }
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
        anyhow::anyhow!(
            "{what} download stalled: the server sent nothing for {} s",
            STALL.as_secs()
        )
    };
    let ask = |from: u64| {
        let request = agent.get(file.url);
        if from == 0 {
            request.call()
        } else {
            request.header("Range", format!("bytes={from}-")).call()
        }
    };
    let mut response = match ask(done) {
        // The part is longer than the file the server has now.
        Err(ureq::Error::StatusCode(416)) if done > 0 => {
            out.set_len(0)?;
            done = 0;
            ask(0)
        }
        response => response,
    }
    .map_err(|e| match e {
        e if timed_out(&e) => stalled(),
        e => anyhow::Error::new(e).context(format!("could not download {what}")),
    })?;
    // Anything but the rest asked for is the whole file.
    if done > 0 && response.status().as_u16() != 206 {
        out.set_len(0)?;
        done = 0;
    }
    let total = response.body().content_length().map(|len| done + len);
    out.seek(SeekFrom::Start(done))?;
    progress(done);
    let mut body = response.body_mut().as_reader();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(anyhow::anyhow!("{what} download cancelled").into());
        }
        let n = match body.read(&mut buf) {
            Ok(n) => n,
            Err(e) if timed_out(&e) => return Err(stalled().into()),
            Err(e) => {
                return Err(anyhow::Error::new(e)
                    .context(format!("{what} download interrupted"))
                    .into());
            }
        };
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])
            .with_context(|| format!("could not write the {what}"))?;
        done += n as u64;
        progress(done);
    }
    out.sync_all()?;
    if total.is_some_and(|t| t != done) {
        return Err(anyhow::anyhow!("{what} download incomplete ({done} bytes)").into());
    }
    // Checked before it takes the file's name, which would make it look
    // downloaded.
    if file.size != 0 && done != file.size {
        return Err(Fetch::Damaged(anyhow::anyhow!(
            "{what} download is the wrong size ({done} bytes, {} published): try again",
            file.size
        )));
    }
    Ok(())
}
