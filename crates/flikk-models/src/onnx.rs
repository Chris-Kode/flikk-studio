//! ONNX Runtime sessions, on the CPU, or on the GPU for the models that
//! gain from it ([`GpuSessions`]). The process has one ONNX Runtime
//! environment, so every model shares it.

use anyhow::{Result, anyhow};
use ort::environment::Environment;
use ort::session::Session;
use ort::session::builder::SessionBuilder;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Sessions kept for one model on the GPU.
const KEPT: usize = 2;

/// Whether this build has a backend for [`GpuSessions`].
const GPU: bool = cfg!(any(
    feature = "coreml",
    feature = "directml",
    feature = "webgpu"
));

/// The process's one ONNX Runtime environment, shared by every model.
fn environment() -> Result<&'static Environment> {
    static ENV: OnceLock<std::result::Result<Environment, String>> = OnceLock::new();
    ENV.get_or_init(|| ort::init().build().map_err(|e| e.to_string()))
        .as_ref()
        .map_err(|e| anyhow!("could not start ONNX Runtime: {e}"))
}

/// A session for `model`; `what` names it in errors ("alignment model").
pub fn session(model: &Path, what: &str) -> Result<Session> {
    builder()?
        .with_execution_providers([cpu()])
        .map_err(|e| anyhow!("{e}"))?
        .commit_from_file(model)
        .map_err(|e| anyhow!("could not load {what} {}: {e}", model.display()))
}

/// A model on the GPU (Core ML, DirectML or WebGPU), with a session per set
/// of free dimensions, the last [`KEPT`] used; on the CPU in other builds,
/// and from the first time the GPU fails to load or run it. Check that what
/// it gives is sound: Core ML can answer NaN without an error.
#[derive(Default)]
pub struct GpuSessions {
    /// Keep what Core ML compiles, for a model that takes long to compile
    /// and is worth the disk space. The other backends keep nothing.
    keep_compiled: bool,
    /// Newest last.
    gpu: Vec<(Vec<(&'static str, i64)>, Session)>,
    cpu: Option<Session>,
}

impl GpuSessions {
    pub fn keeping_compiled() -> Self {
        Self {
            keep_compiled: true,
            ..Self::default()
        }
    }

    /// `run` in a session for `sizes`, once more on the CPU if the GPU fails.
    pub fn run<T>(
        &mut self,
        model: &Path,
        what: &str,
        sizes: &[(&'static str, i64)],
        mut run: impl FnMut(&mut Session) -> Result<T>,
    ) -> Result<T> {
        if GPU && self.cpu.is_none() {
            match self.on_gpu(model, sizes).and_then(&mut run) {
                Ok(value) => return Ok(value),
                Err(e) => {
                    eprintln!("flikk-models: {what} moves to the CPU: {e:#}");
                    self.gpu.clear();
                    // What was compiled may be why.
                    let _ = std::fs::remove_dir_all(compiled(model, sizes));
                }
            }
        }
        if self.cpu.is_none() {
            self.cpu = Some(session(model, what)?);
        }
        run(self.cpu.as_mut().expect("set above"))
    }

    fn on_gpu(&mut self, model: &Path, sizes: &[(&'static str, i64)]) -> Result<&mut Session> {
        match self.gpu.iter().position(|(s, _)| s == sizes) {
            Some(i) => {
                let session = self.gpu.remove(i);
                self.gpu.push(session);
            }
            None => {
                let cache = self.keep_compiled.then(|| compiled(model, sizes));
                let session = gpu_session(model, sizes, cache.as_deref())?;
                if self.gpu.len() == KEPT {
                    self.gpu.remove(0);
                }
                self.gpu.push((sizes.to_vec(), session));
            }
        }
        Ok(&mut self.gpu.last_mut().expect("added above").1)
    }
}

/// Where Core ML keeps `model` compiled for `sizes`: it tells models apart
/// but not their sizes, so each has its own folder.
fn compiled(model: &Path, sizes: &[(&str, i64)]) -> PathBuf {
    let name = model.file_stem().unwrap_or_default().to_string_lossy();
    let sizes: Vec<String> = sizes.iter().map(|(_, size)| size.to_string()).collect();
    flikk_core::paths::cache()
        .join("coreml")
        .join(format!("{name}-{}", sizes.join("x")))
}

/// A session for `model` on the GPU, its free dimensions fixed to `sizes`:
/// the backends run best when they know their shapes, and Core ML needs to.
fn gpu_session(model: &Path, sizes: &[(&str, i64)], cache: Option<&Path>) -> Result<Session> {
    let mut builder = builder()?;
    for (name, size) in sizes {
        builder = builder
            .with_dimension_override(*name, *size)
            .map_err(|e| anyhow!("{e}"))?;
    }
    // DirectML takes neither memory patterns nor parallel execution.
    #[cfg(feature = "directml")]
    let builder = builder
        .with_memory_pattern(false)
        .map_err(|e| anyhow!("{e}"))?
        .with_parallel_execution(false)
        .map_err(|e| anyhow!("{e}"))?;
    builder
        .with_execution_providers([gpu(cache)?, cpu()])
        .map_err(|e| anyhow!("{e}"))?
        .commit_from_file(model)
        .map_err(|e| anyhow!("{e}"))
}

#[cfg(not(any(feature = "coreml", feature = "directml", feature = "webgpu")))]
fn gpu(_: Option<&Path>) -> Result<ort::ep::ExecutionProviderDispatch> {
    Err(anyhow!("this build has no GPU backend"))
}

#[cfg(all(feature = "directml", not(feature = "coreml")))]
fn gpu(_: Option<&Path>) -> Result<ort::ep::ExecutionProviderDispatch> {
    Ok(ort::ep::DirectML::default().build().error_on_failure())
}

#[cfg(all(feature = "webgpu", not(any(feature = "coreml", feature = "directml"))))]
fn gpu(_: Option<&Path>) -> Result<ort::ep::ExecutionProviderDispatch> {
    Ok(ort::ep::WebGPU::default().build().error_on_failure())
}

/// Core ML takes models whole only as ML programs: otherwise it splits them
/// up, and they run slower than on the CPU.
#[cfg(feature = "coreml")]
fn gpu(cache: Option<&Path>) -> Result<ort::ep::ExecutionProviderDispatch> {
    use ort::ep::coreml::{ComputeUnits, ModelFormat};
    let gpu = ort::ep::CoreML::default()
        .with_compute_units(ComputeUnits::CPUAndGPU)
        .with_model_format(ModelFormat::MLProgram)
        .with_static_input_shapes(true);
    let gpu = match cache {
        Some(cache) => {
            std::fs::create_dir_all(cache)?;
            gpu.with_model_cache_dir(cache.display())
        }
        None => gpu,
    };
    Ok(gpu.build().error_on_failure())
}

fn builder() -> Result<SessionBuilder> {
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
        .map_err(|e| anyhow!("{e}"))
}

fn cpu() -> ort::ep::ExecutionProviderDispatch {
    ort::ep::CPU::default().with_arena_allocator(false).build()
}

fn threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().min(8))
        .unwrap_or(4)
}
