//! The release packages ship FFmpeg beside the editor
//! (`scripts/package.sh`). Point `FLIKK_FFMPEG` / `FLIKK_FFPROBE` at that
//! copy, so its hardware encoders are the ones probed, unless they already
//! name a binary. Without a bundled copy, `PATH` is searched as before.

/// Call before any other thread starts: it sets environment variables.
pub fn select() {
    let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.to_path_buf()))
    else {
        return;
    };
    for (var, name) in [("FLIKK_FFMPEG", "ffmpeg"), ("FLIKK_FFPROBE", "ffprobe")] {
        if std::env::var_os(var).is_some_and(|v| !v.is_empty()) {
            continue;
        }
        let path = dir
            .join(name)
            .with_extension(std::env::consts::EXE_EXTENSION);
        if path.is_file() {
            // SAFETY: called from `main` before any other thread exists.
            unsafe { std::env::set_var(var, path) };
        }
    }
}
