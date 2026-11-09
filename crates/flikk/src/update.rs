//! Updates: the latest release on GitHub, downloaded, checked against its
//! SHA-256, unpacked under `paths::updates()` and laid over this install;
//! the new version starts when the user restarts.
//!
//! Only builds the release workflow packages look for updates (it sets
//! `FLIKK_UPDATES` at build time, and `FLIKK_UPDATE_REPO` to its
//! repository); `FLIKK_NO_UPDATES` turns them off at run time. Each release
//! carries one archive per platform, named by [`asset_name`]: the
//! `Flikk.app` bundle on macOS, the folder holding `flikk` elsewhere.

use anyhow::{Context as _, Result, bail};
use sha2::{Digest as _, Sha256};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

const REPO: &str = match option_env!("FLIKK_UPDATE_REPO") {
    Some(repo) => repo,
    None => "Chris-Kode/flikk-studio",
};
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Files an update moved aside or had not finished writing.
const OLD: &str = ".flikk-old";
const NEW: &str = ".flikk-new";
/// In the install while an update swaps its files in: which it replaces
/// and which it adds, so a swap cut short can be undone.
const JOURNAL: &str = "update.flikk-journal";

/// A version newer than this one, with this platform's archive.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    /// Its page on GitHub, to download it from by hand.
    pub page: String,
    /// Whether this copy can lay it over itself: not when its folder needs
    /// an administrator (Program Files, /opt).
    pub installable: bool,
    archive: String,
    checksum: Option<String>,
}

/// The executable, as it was when the app started: on Linux the path of a
/// replaced one reads "… (deleted)".
static EXE: OnceLock<Option<PathBuf>> = OnceLock::new();
static RELAUNCH: AtomicBool = AtomicBool::new(false);
/// Held while an update installs, or its leftovers are swept.
static BUSY: Mutex<()> = Mutex::new(());

pub fn enabled() -> bool {
    option_env!("FLIKK_UPDATES").is_some() && std::env::var_os("FLIKK_NO_UPDATES").is_none()
}

/// At launch: remember the executable and, off the UI thread, remove what
/// the last update left behind.
pub fn init() {
    let exe = exe().map(Path::to_path_buf);
    if !enabled() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("flikk-update-sweep".into())
        .spawn(move || {
            // The version being replaced may still be on its way out.
            std::thread::sleep(Duration::from_secs(10));
            let _busy = BUSY.lock();
            let _ = std::fs::remove_dir_all(flikk_core::paths::updates());
            if let Some(root) = exe.as_deref().and_then(install_root) {
                // An update cut short (a crash, a failed undo) is undone
                // first; the files moved aside are kept until it is.
                if restore(&root) {
                    sweep(&root);
                }
            }
        });
}

fn exe() -> Option<&'static Path> {
    EXE.get_or_init(|| {
        let exe = std::env::current_exe().ok()?;
        // Windows would answer with a `\\?\` path.
        if cfg!(windows) {
            return Some(exe);
        }
        Some(exe.canonicalize().unwrap_or(exe))
    })
    .as_deref()
}

/// What an update replaces: the app bundle on macOS, the executable's
/// folder elsewhere.
fn install_root(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?;
    if cfg!(target_os = "macos") {
        return dir
            .ends_with("Contents/MacOS")
            .then(|| dir.parent()?.parent().map(Path::to_path_buf))
            .flatten();
    }
    Some(dir.to_path_buf())
}

/// This platform's archive in the release of `version`.
pub fn asset_name(version: &str) -> String {
    let (os, ext) = match std::env::consts::OS {
        "windows" => ("windows", "zip"),
        os => (os, "tar.gz"),
    };
    format!("flikk-{version}-{os}-{}.{ext}", std::env::consts::ARCH)
}

/// `1.2.3` from `v1.2.3`; pre-release and build suffixes are ignored.
fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let core = text.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

fn agent() -> ureq::Agent {
    use ureq::tls::{RootCerts, TlsConfig};
    ureq::Agent::config_builder()
        .tls_config(
            TlsConfig::builder()
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_body(None)
        .user_agent(format!("flikk/{VERSION}"))
        .build()
        .into()
}

/// The latest release when it is newer than this build and has this
/// platform's archive; `None` otherwise, or while its archives upload.
pub fn latest() -> Result<Option<Release>> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = agent()
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .context("could not reach GitHub")?
        .body_mut()
        .read_to_string()
        .context("could not read the latest release")?;
    let json: serde_json::Value = serde_json::from_str(&body)?;
    let Some(tag) = json["tag_name"].as_str() else {
        return Ok(None);
    };
    let (Some(latest), Some(current)) = (parse_version(tag), parse_version(VERSION)) else {
        return Ok(None);
    };
    if latest <= current || json["draft"].as_bool() == Some(true) {
        return Ok(None);
    }
    let version = format!("{}.{}.{}", latest.0, latest.1, latest.2);
    let name = asset_name(&version);
    let url_of = |wanted: &str| {
        json["assets"].as_array()?.iter().find_map(|asset| {
            (asset["name"].as_str() == Some(wanted))
                .then(|| asset["browser_download_url"].as_str().map(str::to_owned))
                .flatten()
        })
    };
    let Some(archive) = url_of(&name) else {
        return Ok(None);
    };
    let page = json["html_url"].as_str().map_or_else(
        || format!("https://github.com/{REPO}/releases/latest"),
        str::to_owned,
    );
    Ok(Some(Release {
        version,
        page,
        installable: installable(),
        archive,
        checksum: url_of(&format!("{name}.sha256")),
    }))
}

/// Download `release`, check it and lay it over this install. `progress`
/// counts the download in thousandths.
pub fn install(release: &Release, progress: &AtomicU32) -> Result<()> {
    let _busy = BUSY.lock();
    let exe = exe().context("could not find Flikk's executable")?;
    let root = install_root(exe).context("Flikk is not running from an installed copy")?;
    writable(&root)?;
    if !restore(&root) {
        bail!("an earlier update could not be undone yet; restart Flikk and try again");
    }

    let dir = flikk_core::paths::updates();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let archive = dir.join(asset_name(&release.version));
    download(&release.archive, &archive, progress)?;
    if let Some(url) = &release.checksum {
        verify(&archive, url)?;
    }

    let unpacked = dir.join("unpacked");
    std::fs::create_dir_all(&unpacked)?;
    unpack(&archive, &unpacked)?;
    let payload = single_dir(&unpacked)?;
    lay_over(&payload, &root)?;
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// Whether an update can be laid over this copy.
pub fn installable() -> bool {
    exe()
        .and_then(install_root)
        .is_some_and(|root| writable(&root).is_ok())
}

/// Fail early, before any file is replaced, where the install can't be
/// written to (a system folder).
fn writable(root: &Path) -> Result<()> {
    let probe = root.join(format!("update-probe{NEW}"));
    std::fs::File::create(&probe)
        .and_then(|_| std::fs::remove_file(&probe))
        .with_context(|| format!("{} can't be written to", root.display()))
}

fn download(url: &str, dest: &Path, progress: &AtomicU32) -> Result<()> {
    let mut response = agent()
        .get(url)
        .call()
        .context("could not download the update")?;
    let total = response.body().content_length();
    let mut body = response.body_mut().as_reader();
    let mut out = std::fs::File::create(dest)
        .with_context(|| format!("could not write {}", dest.display()))?;
    let mut buf = vec![0u8; 1 << 20];
    let mut done = 0u64;
    loop {
        let n = body
            .read(&mut buf)
            .context("the download was interrupted")?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        done += n as u64;
        if let Some(total) = total.filter(|t| *t > 0) {
            progress.store((done * 1000 / total).min(1000) as u32, Ordering::Relaxed);
        }
    }
    out.sync_all()?;
    if total.is_some_and(|t| t != done) {
        bail!("the download is incomplete");
    }
    Ok(())
}

/// Check `file` against the `sha256sum` line at `url`.
fn verify(file: &Path, url: &str) -> Result<()> {
    let line = agent()
        .get(url)
        .call()
        .context("could not download the update's checksum")?
        .body_mut()
        .read_to_string()?;
    let expected = line.split_whitespace().next().unwrap_or_default();
    let mut hasher = Sha256::new();
    std::io::copy(&mut std::fs::File::open(file)?, &mut hasher)?;
    let actual: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if !actual.eq_ignore_ascii_case(expected) {
        bail!("the download is damaged (checksum mismatch)");
    }
    Ok(())
}

/// With the system's `tar`: GNU tar on Linux, bsdtar on macOS and on
/// Windows, where it reads zip archives too.
fn unpack(archive: &Path, into: &Path) -> Result<()> {
    let tar = match std::env::var_os("SystemRoot") {
        Some(root) if cfg!(windows) => Path::new(&root).join("System32").join("tar.exe"),
        _ => PathBuf::from("tar"),
    };
    let output = flikk_os::process::command(&tar)
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .output()
        .with_context(|| format!("could not run {}", tar.display()))?;
    if !output.status.success() {
        bail!(
            "could not unpack the update: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// The folder an archive holds everything in.
fn single_dir(dir: &Path) -> Result<PathBuf> {
    let entries: Vec<_> = std::fs::read_dir(dir)?.filter_map(Result::ok).collect();
    match entries.as_slice() {
        [only] if only.path().is_dir() => Ok(only.path()),
        _ => bail!("the update's archive is not laid out as expected"),
    }
}

/// Copy every file under `from` to the same place under `to`, all or
/// nothing. Every file is first written beside its target; then each
/// target is moved aside and the new file renamed into place, with a
/// journal of what changes. If any of that fails, what was swapped is put
/// back; a swap cut short by a crash is put back at the next launch. The
/// files moved aside are removed then too (a running executable or a
/// loaded library can be renamed, but on Windows not replaced).
fn lay_over(from: &Path, to: &Path) -> Result<()> {
    let mut files = Vec::new();
    list_files(from, Path::new(""), &mut files)?;
    let mut made = Vec::new();
    let staged = files.iter().try_for_each(|file| -> Result<()> {
        let dst = to.join(file);
        if let Some(dir) = dst.parent() {
            create_dirs(dir, &mut made)?;
        }
        // Left by an update that finished: what is moved aside now goes
        // there.
        let _ = std::fs::remove_file(suffixed(&dst, OLD));
        let new = suffixed(&dst, NEW);
        std::fs::copy(from.join(file), &new)
            .with_context(|| format!("could not write {}", new.display()))?;
        Ok(())
    });
    let unstage = |made: &[PathBuf]| {
        for file in &files {
            let _ = std::fs::remove_file(suffixed(&to.join(file), NEW));
        }
        for dir in made.iter().rev() {
            let _ = std::fs::remove_dir(dir);
        }
    };
    if let Err(e) = staged {
        unstage(&made);
        return Err(e);
    }
    let journal: String = files
        .iter()
        .map(|file| {
            let kind = if to.join(file).exists() { "old" } else { "new" };
            format!("{kind}\t{}\n", file.display())
        })
        .collect();
    if let Err(e) = std::fs::write(to.join(JOURNAL), journal) {
        unstage(&made);
        return Err(e).context("could not start replacing Flikk's files");
    }
    for file in &files {
        if let Err(e) = swap(&to.join(file)) {
            let undone = restore(to);
            unstage(&made);
            return Err(e).context(if undone {
                "the update was undone"
            } else {
                "the update could not be fully undone; restart Flikk to finish putting it back"
            });
        }
    }
    let _ = std::fs::remove_file(to.join(JOURNAL));
    Ok(())
}

/// The files under `dir`, relative to its root, as `rel` within it.
fn list_files(dir: &Path, rel: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let (path, rel) = (entry.path(), rel.join(entry.file_name()));
        if path.is_dir() {
            list_files(&path, &rel, out)?;
        } else {
            out.push(rel);
        }
    }
    Ok(())
}

/// Create `dir` and its missing parents, noting each one made.
fn create_dirs(dir: &Path, made: &mut Vec<PathBuf>) -> Result<()> {
    if dir.exists() {
        return Ok(());
    }
    if let Some(parent) = dir.parent() {
        create_dirs(parent, made)?;
    }
    std::fs::create_dir(dir).with_context(|| format!("could not create {}", dir.display()))?;
    made.push(dir.to_path_buf());
    Ok(())
}

/// Put the staged `<dst>.flikk-new` in place, moving `dst` aside first.
fn swap(dst: &Path) -> Result<()> {
    if dst.exists() {
        std::fs::rename(dst, suffixed(dst, OLD))
            .with_context(|| format!("could not replace {}", dst.display()))?;
    }
    std::fs::rename(suffixed(dst, NEW), dst)
        .with_context(|| format!("could not replace {}", dst.display()))
}

/// Undo the swap the journal in `root` describes, if there is one: put
/// back each file moved aside and remove each file added. Whether nothing
/// is left to undo.
fn restore(root: &Path) -> bool {
    let journal = root.join(JOURNAL);
    let Ok(text) = std::fs::read_to_string(&journal) else {
        return !journal.exists();
    };
    let mut undone = true;
    for line in text.lines() {
        let Some((kind, file)) = line.split_once('\t') else {
            continue;
        };
        let dst = root.join(file);
        let old = suffixed(&dst, OLD);
        match kind {
            "old" if old.exists() => {
                // The update's file goes where staged files are swept.
                if dst.exists() {
                    let aside = suffixed(&dst, NEW);
                    let _ = std::fs::remove_file(&aside);
                    if std::fs::rename(&dst, &aside).is_err() {
                        undone = false;
                        continue;
                    }
                }
                undone &= std::fs::rename(&old, &dst).is_ok();
            }
            "new" if dst.exists() => undone &= std::fs::remove_file(&dst).is_ok(),
            _ => {}
        }
    }
    if undone {
        let _ = std::fs::remove_file(&journal);
    }
    undone
}

fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Remove files an update moved aside or left half-written.
fn sweep(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            sweep(&path);
        } else if path
            .to_str()
            .is_some_and(|p| p.ends_with(OLD) || p.ends_with(NEW))
        {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Start the installed version once this one has quit.
pub fn relaunch_on_quit() {
    RELAUNCH.store(true, Ordering::Relaxed);
}

/// From the app's quit handler, after the session is written.
pub fn relaunch_if_asked() {
    if !RELAUNCH.load(Ordering::Relaxed) {
        return;
    }
    let Some(exe) = exe() else {
        return;
    };
    // A second instance of the bundle, as Finder would start it.
    if cfg!(target_os = "macos")
        && let Some(bundle) = install_root(exe)
    {
        let _ = std::process::Command::new("open")
            .arg("-n")
            .arg(bundle)
            .spawn();
        return;
    }
    let _ = std::process::Command::new(exe).spawn();
}
