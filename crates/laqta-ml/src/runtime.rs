//! The model runtime (ADR 023): ONNX Runtime loading, the session cache with its release rule, and
//! downloads of catalog entries through laqta-http's verified fetch.

use std::collections::HashMap;
use std::fmt;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use futures_channel::oneshot;
use laqta_ports::{BoxFuture, PortError};
use ort::environment::Environment;
use ort::ep::ExecutionProviderDispatch;
use ort::session::Session;

use crate::catalog::{ModelEntry, Task};
use crate::error::MlError;

/// Which execution provider a session uses.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ExecutionProvider {
    /// A GPU provider when one is available, otherwise the CPU.
    #[default]
    Auto,
    Cpu,
    /// As `Auto`. A GPU provider that fails to load falls back to the CPU rather than failing.
    Gpu,
}

/// Download progress: bytes so far, and the total when it is known.
pub type Progress = Arc<dyn Fn(u64, Option<u64>) + Send + Sync>;

/// ONNX Runtime allows one environment per process, and the slot is not freed when an environment is
/// dropped, so the environment lives in a static.
static ENVIRONMENT: OnceLock<Environment> = OnceLock::new();
static ENVIRONMENT_INIT: Mutex<()> = Mutex::new(());

/// Loads ONNX Runtime from `lib_path` and creates the process environment. The first successful load
/// wins; later calls return the same environment.
fn load_environment(lib_path: &Path) -> Result<&'static Environment, MlError> {
    if let Some(environment) = ENVIRONMENT.get() {
        return Ok(environment);
    }
    let _guard = ENVIRONMENT_INIT.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(environment) = ENVIRONMENT.get() {
        return Ok(environment);
    }
    if !lib_path.is_file() {
        return Err(MlError::OrtLibraryMissing(lib_path.to_path_buf()));
    }
    let environment = ort::init_from(lib_path).map_err(|error| MlError::Ort(error.to_string()))?.build().map_err(ort_error)?;
    // The lock is held and the cell was empty above, so this cannot lose a race.
    let _ = ENVIRONMENT.set(environment);
    ENVIRONMENT.get().ok_or(MlError::RuntimeNotLoaded)
}

/// The environment loaded by a `Runtime` in this process, or `RuntimeNotLoaded`.
pub(crate) fn loaded_environment() -> Result<&'static Environment, MlError> {
    ENVIRONMENT.get().ok_or(MlError::RuntimeNotLoaded)
}

pub(crate) fn ort_error<R>(error: ort::Error<R>) -> MlError {
    MlError::Ort(error.to_string())
}

/// Runs `work` on a new thread and resolves with its result, so the caller's executor never waits on a
/// model. This is the pattern `ModelOcr` uses. A thread that fails to start is reported as a port error.
pub(crate) fn run_on_thread<T: Send + 'static>(
    name: &str,
    work: impl FnOnce() -> Result<T, MlError> + Send + 'static,
) -> BoxFuture<'static, Result<T, PortError>> {
    let (sender, receiver) = oneshot::channel();
    let spawned = std::thread::Builder::new().name(name.into()).spawn(move || {
        // The receiver is gone only if the caller stopped waiting, so the result is not needed.
        let _ = sender.send(work());
    });
    if let Err(error) = spawned {
        return Box::pin(async move { Err(PortError::Failed(format!("cannot start the model thread: {error}"))) });
    }
    Box::pin(async move {
        match receiver.await {
            Ok(result) => result.map_err(PortError::from),
            Err(_) => Err(PortError::Failed("the model thread stopped without a result".into())),
        }
    })
}

struct Loaded {
    model: String,
    session: Session,
    last_used: Instant,
}

/// The model runtime for one process: where ONNX Runtime and the models live, and the sessions that
/// are loaded now. At most one session is loaded per task (ADR 023).
pub struct Runtime {
    lib_path: PathBuf,
    models_dir: PathBuf,
    provider: ExecutionProvider,
    sessions: Mutex<HashMap<Task, Loaded>>,
}

impl fmt::Debug for Runtime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Runtime")
            .field("lib_path", &self.lib_path)
            .field("models_dir", &self.models_dir)
            .field("provider", &self.provider)
            .finish_non_exhaustive()
    }
}

impl Runtime {
    /// `lib_path` is the ONNX Runtime shared library. It is loaded on first use, not here.
    pub fn new(lib_path: impl Into<PathBuf>, models_dir: impl Into<PathBuf>, provider: ExecutionProvider) -> Self {
        Runtime { lib_path: lib_path.into(), models_dir: models_dir.into(), provider, sessions: Mutex::new(HashMap::new()) }
    }

    pub fn models_dir(&self) -> &Path {
        &self.models_dir
    }

    /// Loads ONNX Runtime now, if it is not loaded yet.
    pub fn ensure_ort(&self) -> Result<(), MlError> {
        load_environment(&self.lib_path).map(|_| ())
    }

    /// The model file for a catalog id.
    pub fn model_path(&self, id: &str) -> PathBuf {
        self.models_dir.join(format!("{id}.onnx"))
    }

    /// The dictionary file for a recogniser's catalog id.
    pub fn dict_path(&self, id: &str) -> PathBuf {
        self.models_dir.join(format!("{id}.dict.txt"))
    }

    /// True if the files for a catalog entry are on disk. A recogniser also needs its dictionary.
    pub fn is_installed(&self, entry: &ModelEntry) -> bool {
        self.model_path(&entry.id).is_file() && (entry.task != Task::OcrRec || self.dict_path(&entry.id).is_file())
    }

    /// Runs `run` on the session for `task`, loading `model_id` first when it is not loaded. Loading
    /// a different model for the same task releases the previous session before the new one loads.
    pub(crate) fn with_session<R>(
        &self,
        task: Task,
        model_id: &str,
        run: impl FnOnce(&mut Session) -> Result<R, MlError>,
    ) -> Result<R, MlError> {
        let environment = load_environment(&self.lib_path)?;
        let mut sessions = self.lock_sessions();
        let mut loaded = match sessions.remove(&task) {
            Some(loaded) if loaded.model == model_id => loaded,
            _ => Loaded {
                model: model_id.to_owned(),
                session: build_session(environment, &self.model_path(model_id), self.provider)?,
                last_used: Instant::now(),
            },
        };
        loaded.last_used = Instant::now();
        let result = run(&mut loaded.session);
        sessions.insert(task, loaded);
        result
    }

    /// Drops every session that has not been used for longer than `after`. Returns true while any
    /// session is still loaded, so the caller knows whether to keep its release timer armed.
    pub fn release_idle(&self, after: Duration) -> bool {
        let mut sessions = self.lock_sessions();
        let idle = idle_keys(sessions.iter().map(|(task, loaded)| (*task, loaded.last_used)), Instant::now(), after);
        for task in idle {
            sessions.remove(&task);
        }
        !sessions.is_empty()
    }

    /// Downloads a catalog entry into the models directory, and a recogniser's dictionary with it. Each
    /// file is checked against its pinned SHA-256 before it appears under its final name. An entry with
    /// a placeholder hash is refused before anything is fetched.
    pub async fn install(&self, entry: &ModelEntry, progress: Progress, cancel: Arc<AtomicBool>) -> Result<(), MlError> {
        if entry.is_pending() {
            return Err(MlError::PendingHash(entry.id.clone()));
        }
        laqta_http::fetch_verified(&entry.url, &self.model_path(&entry.id), &entry.sha256, progress.clone(), cancel.clone()).await?;
        if entry.task == Task::OcrRec {
            let (Some(url), Some(hash)) = (entry.dict_url.as_deref(), entry.dict_sha256.as_deref()) else {
                return Err(MlError::Catalog(format!("recogniser `{}` has no dictionary", entry.id)));
            };
            laqta_http::fetch_verified(url, &self.dict_path(&entry.id), hash, progress, cancel).await?;
        }
        Ok(())
    }

    fn lock_sessions(&self) -> MutexGuard<'_, HashMap<Task, Loaded>> {
        self.sessions.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The keys of entries whose last use is older than `after`. Pure, so the release rule can be tested
/// without ONNX Runtime.
pub(crate) fn idle_keys<K>(entries: impl IntoIterator<Item = (K, Instant)>, now: Instant, after: Duration) -> Vec<K> {
    entries.into_iter().filter(|(_, last_used)| now.saturating_duration_since(*last_used) > after).map(|(key, _)| key).collect()
}

/// Execution providers to try before the CPU. DirectML (Windows) and CUDA (Linux) are compiled only
/// with the `directml` and `cuda` features of `ort`, which this crate does not enable yet. Until they
/// are, `Auto` and `Gpu` run on the CPU.
fn gpu_providers(_provider: ExecutionProvider) -> Vec<ExecutionProviderDispatch> {
    Vec::new()
}

fn build_session(environment: &Environment, path: &Path, provider: ExecutionProvider) -> Result<Session, MlError> {
    let threads = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
    let gpu = gpu_providers(provider);
    if !gpu.is_empty() {
        match commit(environment, path, threads, &gpu) {
            Ok(session) => return Ok(session),
            Err(error) => tracing::warn!(%error, "a GPU provider failed to load; using the CPU"),
        }
    }
    commit(environment, path, threads, &[])
}

fn commit(environment: &Environment, path: &Path, threads: usize, providers: &[ExecutionProviderDispatch]) -> Result<Session, MlError> {
    let mut builder = Session::builder(environment)
        .map_err(ort_error)?
        .with_intra_threads(threads)
        .map_err(ort_error)?
        .with_execution_providers(providers)
        .map_err(ort_error)?;
    builder.commit_from_file(path).map_err(ort_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::catalog;

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        let mut future = std::pin::pin!(future);
        loop {
            if let std::task::Poll::Ready(output) = future.as_mut().poll(&mut context) {
                return output;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn runtime(lib: &str) -> Runtime {
        Runtime::new(lib, "/nonexistent-models", ExecutionProvider::Cpu)
    }

    #[test]
    fn idle_keys_picks_only_entries_older_than_the_limit() {
        let now = Instant::now();
        let old = now.checked_sub(Duration::from_secs(10)).expect("instant in range");
        let recent = now.checked_sub(Duration::from_secs(1)).expect("instant in range");
        let idle = idle_keys([("old", old), ("recent", recent)], now, Duration::from_secs(5));
        assert_eq!(idle, vec!["old"]);
    }

    #[test]
    fn idle_keys_keeps_an_entry_used_exactly_at_the_limit() {
        let now = Instant::now();
        let at_limit = now.checked_sub(Duration::from_secs(5)).expect("instant in range");
        assert!(idle_keys([("x", at_limit)], now, Duration::from_secs(5)).is_empty());
    }

    #[test]
    fn release_idle_on_an_empty_runtime_reports_nothing_loaded() {
        assert!(!runtime("/nonexistent-lib.so").release_idle(Duration::ZERO));
    }

    #[test]
    fn model_and_dictionary_paths_share_the_models_directory() {
        let runtime = Runtime::new("/lib/onnxruntime.so", "/models", ExecutionProvider::Auto);
        assert_eq!(runtime.model_path("ppocr-v5-mobile-det"), Path::new("/models/ppocr-v5-mobile-det.onnx"));
        assert_eq!(runtime.dict_path("ppocr-v5-mobile-rec-multi"), Path::new("/models/ppocr-v5-mobile-rec-multi.dict.txt"));
    }

    #[test]
    fn a_missing_library_is_reported_before_any_ort_call() {
        let runtime = runtime("/definitely/not/here/libonnxruntime.so");
        let error = runtime.ensure_ort().expect_err("no library");
        assert!(matches!(error, MlError::OrtLibraryMissing(_)), "{error}");
        assert!(matches!(loaded_environment(), Err(MlError::RuntimeNotLoaded)));
    }

    #[test]
    fn install_refuses_a_pending_hash_without_fetching() {
        let mut entry = catalog().model("ppocr-v5-mobile-det").expect("catalog entry").clone();
        entry.sha256 = "pending-owner".to_owned();
        let runtime = runtime("/nonexistent-lib.so");
        let progress: Progress = Arc::new(|_, _| {});
        let result = block_on(runtime.install(&entry, progress, Arc::new(AtomicBool::new(false))));
        match result {
            Err(MlError::PendingHash(id)) => assert_eq!(id, "ppocr-v5-mobile-det"),
            other => panic!("expected a pending-hash refusal, got {other:?}"),
        }
    }
}
