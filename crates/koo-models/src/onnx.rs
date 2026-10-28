//! ONNX Runtime sessions for the models run on the CPU. The process has one
//! ONNX Runtime environment, so every model shares it.

use anyhow::{Result, anyhow};
use ort::environment::Environment;
use ort::session::Session;
use std::path::Path;
use std::sync::OnceLock;

/// The process's one ONNX Runtime environment, shared by every model.
fn environment() -> Result<&'static Environment> {
    static ENV: OnceLock<std::result::Result<Environment, String>> = OnceLock::new();
    ENV.get_or_init(|| ort::init().build().map_err(|e| e.to_string()))
        .as_ref()
        .map_err(|e| anyhow!("could not start ONNX Runtime: {e}"))
}

/// A session for `model`; `what` names it in errors ("alignment model").
pub fn session(model: &Path, what: &str) -> Result<Session> {
    Session::builder(environment()?)
        .map_err(|e| anyhow!("{e}"))?
        .with_intra_threads(threads())
        .map_err(|e| anyhow!("{e}"))?
        // Idle threads would spin and slow a Whisper job running next to
        // this one.
        .with_intra_op_spinning(false)
        .map_err(|e| anyhow!("{e}"))?
        // Repacked weights cost memory for no speed gain here, and an
        // arena would keep each run's working memory (500 MB for 30 s of
        // audio in the aligner) for good.
        .with_prepacking(false)
        .map_err(|e| anyhow!("{e}"))?
        .with_execution_providers([ort::ep::CPU::default().with_arena_allocator(false).build()])
        .map_err(|e| anyhow!("{e}"))?
        .commit_from_file(model)
        .map_err(|e| anyhow!("could not load {what} {}: {e}", model.display()))
}

fn threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().min(8))
        .unwrap_or(4)
}
