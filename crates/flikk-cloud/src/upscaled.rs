//! An upscaled video with its source's own sound. Upscalers make the
//! picture; some drop the sound or encode it again, so the picture made is
//! put together with the source's sound, as it was.

use flikk_core::cloud::UpscaleSource;
use flikk_os::process::{ChildGuard, command, ffmpeg};
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// `made` with `source`'s sound, as `<stem>.mov`; `made` itself when the
/// source has none, or when FFmpeg can't put them together: what was made
/// is paid for, so it is kept either way.
pub(crate) fn with_source_sound(made: PathBuf, source: &UpscaleSource, stem: &Path) -> PathBuf {
    if !source.sound {
        return made;
    }
    let mut out = stem.as_os_str().to_owned();
    out.push(".mov");
    let out = PathBuf::from(out);
    let mut part = out.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    // The sound as it is, else as AAC where MOV can't hold it as it is.
    let sounds: [&[&str]; 2] = [&["-c:a", "copy"], &["-c:a", "aac", "-b:a", "320k"]];
    for sound in sounds {
        let _ = std::fs::remove_file(&part);
        if mux(&made, &source.path, sound, &part) && settle(&part, &made, &out) {
            return out;
        }
    }
    let _ = std::fs::remove_file(&part);
    made
}

/// Write `picture`'s video and `sound`'s audio, encoded as `audio` says,
/// to `part` as MOV; whether FFmpeg did.
fn mux(picture: &Path, sound: &Path, audio: &[&str], part: &Path) -> bool {
    let mut ffmpeg = command(ffmpeg());
    ffmpeg
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(picture)
        .arg("-i")
        .arg(sound)
        .args(["-map", "0:v:0", "-map", "1:a", "-c:v", "copy"])
        .args(audio)
        .args(["-f", "mov"])
        .arg(part)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    ChildGuard::spawn(&mut ffmpeg)
        .and_then(|mut child| child.wait())
        .is_ok_and(|status| status.success())
}

/// Put `part` at `out` in place of `made`, which this generation wrote and
/// `out` may be; whether it is there.
fn settle(part: &Path, made: &Path, out: &Path) -> bool {
    if out == made {
        return std::fs::rename(part, out).is_ok();
    }
    if crate::settle(part, out).is_err() {
        return false;
    }
    let _ = std::fs::remove_file(made);
    true
}
