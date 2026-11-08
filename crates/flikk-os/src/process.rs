//! Child processes that never outlive the work they do, and where to find
//! FFmpeg.

use std::ffi::OsStr;
use std::io;
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

/// A command for `program` with no stdin (it never reads the editor's, nor
/// the MCP server's stream) and, on Windows, no console window. Set stdin
/// again for a process that is fed through a pipe.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    command.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// A spawned child, killed and reaped when dropped: on an early return, an
/// error or a panic it never goes on working for nobody, nor stays a
/// zombie. Dropping one already waited for does nothing. Use it as the
/// [`Child`] it derefs to.
#[derive(Debug)]
pub struct ChildGuard(Child);

impl ChildGuard {
    /// Spawn `command` guarded.
    pub fn spawn(command: &mut Command) -> io::Result<Self> {
        command.spawn().map(Self)
    }
}

impl From<Child> for ChildGuard {
    fn from(child: Child) -> Self {
        Self(child)
    }
}

impl Deref for ChildGuard {
    type Target = Child;

    fn deref(&self) -> &Child {
        &self.0
    }
}

impl DerefMut for ChildGuard {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        // Both are no-ops once it has been waited for.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The `ffmpeg` binary: `$FLIKK_FFMPEG` when set, else `ffmpeg` from `PATH`.
/// The app points the variable at the copy packaged beside it.
pub fn ffmpeg() -> PathBuf {
    binary("FLIKK_FFMPEG", "ffmpeg")
}

/// The `ffprobe` binary: `$FLIKK_FFPROBE` when set, else `ffprobe` from
/// `PATH`.
pub fn ffprobe() -> PathBuf {
    binary("FLIKK_FFPROBE", "ffprobe")
}

fn binary(variable: &str, name: &str) -> PathBuf {
    std::env::var_os(variable)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(name))
}
