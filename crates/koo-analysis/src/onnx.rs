//! ONNX Runtime, shared by every model run with it. It allows one
//! environment per process, so the first model loaded makes it for all.

use anyhow::{Result, anyhow};
use ort::environment::Environment;
use ort::session::Session;
use std::path::Path;
use std::sync::OnceLock;

fn environment() -> Result<Environment> {
    static ENV: OnceLock<Result<Environment, String>> = OnceLock::new();
    ENV.get_or_init(|| {
        ort::init()
            .build()
            .map_err(|e| format!("could not start ONNX Runtime: {e}"))
    })
    .clone()
    .map_err(|e| anyhow!(e))
}

/// Load the `what` model at `path` to run on the CPU.
pub fn session(path: &Path, what: &str) -> Result<Session> {
    let env = environment()?;
    Session::builder(&env)
        .map_err(|e| anyhow!("{e}"))?
        .with_intra_threads(threads())
        .map_err(|e| anyhow!("{e}"))?
        // Idle threads would spin and slow a Whisper job running next to
        // this one.
        .with_intra_op_spinning(false)
        .map_err(|e| anyhow!("{e}"))?
        // Repacked weights cost the aligner 180 MB for no speed gain, and
        // an arena would keep each run's working memory (500 MB for 30 s
        // of audio in the aligner) for good.
        .with_prepacking(false)
        .map_err(|e| anyhow!("{e}"))?
        .with_execution_providers([ort::ep::CPU::default().with_arena_allocator(false).build()])
        .map_err(|e| anyhow!("{e}"))?
        .commit_from_file(path)
        .map_err(|e| anyhow!("could not load {what} {}: {e}", path.display()))
}

fn threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().min(8))
        .unwrap_or(4)
}
