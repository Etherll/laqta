//! The model manager's service (ADR 023): catalog and custom models, downloads, removal and
//! custom-model validation, over `laqta-ml`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use laqta_domain::settings::{CustomModel, ModelOptions, ModelProvider};
use laqta_domain::workflow::ToastOptions;
use laqta_ml::families::Family;
use laqta_ml::runtime::{ExecutionProvider, Runtime};
use laqta_ml::{Task, catalog};
use laqta_ports::{BoxFuture, Clock, Notifier, Spawner, Toast};
use laqta_ui::{ModelProgress, ModelRow, ModelService};

/// The settings' model choices, shared with the OCR adapter and updated on save.
pub type SharedModelOptions = Arc<std::sync::RwLock<ModelOptions>>;

pub fn provider(p: ModelProvider) -> ExecutionProvider {
    match p {
        ModelProvider::Auto => ExecutionProvider::Auto,
        ModelProvider::Cpu => ExecutionProvider::Cpu,
        ModelProvider::Gpu => ExecutionProvider::Gpu,
    }
}

pub fn models_dir(data: &Path) -> PathBuf {
    data.join("models")
}

/// A runtime over `<data>/models` with the chosen provider. ONNX Runtime may still be missing.
pub fn runtime(data: &Path, options: &ModelOptions) -> Option<Arc<Runtime>> {
    let dir = models_dir(data);
    let ort = laqta_ml::install::ort_for_host()?;
    Some(Arc::new(Runtime::new(laqta_ml::install::library_path(&dir, ort), dir, provider(options.provider))))
}

/// How long a loaded model may sit unused before its sessions are released (ARCHITECTURE §1.4).
const MODEL_RELEASE: Duration = Duration::from_secs(60);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Releases the ONNX Runtime sessions (the model, its working memory and its threads) of every
/// watched runtime once they go unused for 60 s. One timer runs while any session is loaded, and
/// none when nothing is (ARCHITECTURE §1.4). It ticks every 60 s, so a session goes 60 to 120 s
/// after its last use. The ONNX Runtime library itself stays loaded.
pub struct ModelReleaser {
    clock: Arc<dyn Clock>,
    spawner: Arc<dyn Spawner>,
    state: Arc<ReleaseState>,
}

#[derive(Default)]
struct ReleaseState {
    runtimes: Mutex<Vec<Weak<Runtime>>>,
    /// Bumped by every `watch`, so the timer doesn't disarm across a use it hasn't seen.
    uses: std::sync::atomic::AtomicU64,
    armed: Mutex<bool>,
}

impl ModelReleaser {
    pub fn new(clock: Arc<dyn Clock>, spawner: Arc<dyn Spawner>) -> Arc<Self> {
        Arc::new(ModelReleaser { clock, spawner, state: Arc::default() })
    }

    /// Call after each model run: keeps `runtime` on the list and the release timer armed.
    pub fn watch(&self, runtime: &Arc<Runtime>) {
        {
            let mut runtimes = lock(&self.state.runtimes);
            runtimes.retain(|w| w.strong_count() > 0);
            if !runtimes.iter().any(|w| std::ptr::eq(w.as_ptr(), Arc::as_ptr(runtime))) {
                runtimes.push(Arc::downgrade(runtime));
            }
        }
        self.state.uses.fetch_add(1, Ordering::SeqCst);
        {
            let mut armed = lock(&self.state.armed);
            if *armed {
                return;
            }
            *armed = true;
        }
        let (state, clock, spawner) = (self.state.clone(), self.clock.clone(), self.spawner.clone());
        self.spawner.spawn(Box::pin(async move {
            loop {
                let seen = state.uses.load(Ordering::SeqCst);
                clock.sleep(MODEL_RELEASE).await;
                let runtimes: Vec<Arc<Runtime>> = lock(&state.runtimes).iter().filter_map(Weak::upgrade).collect();
                let loaded = Arc::new(AtomicBool::new(false));
                let any = loaded.clone();
                // `release_idle` waits for a model that is running, so it runs on the blocking pool.
                spawner
                    .run_blocking(Box::new(move || {
                        let mut still = false;
                        for runtime in &runtimes {
                            still |= runtime.release_idle(MODEL_RELEASE);
                        }
                        any.store(still, Ordering::SeqCst);
                    }))
                    .await;
                if loaded.load(Ordering::SeqCst) {
                    continue;
                }
                let mut armed = lock(&state.armed);
                if state.uses.load(Ordering::SeqCst) != seen {
                    continue; // used meanwhile: keep timing that use
                }
                *armed = false;
                drop(armed);
                tracing::debug!("model sessions released");
                trim_heap();
                return;
            }
        }));
    }
}

/// Hands memory the allocator keeps after the models are released back to the system. glibc keeps
/// freed pages in its per-thread arenas (report item 5); other allocators return them on their own.
fn trim_heap() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: `malloc_trim` only walks glibc's own arenas; any pad value is valid.
    unsafe {
        libc::malloc_trim(0);
    }
}

/// A key, the engine built for it, and the runtime the engine runs on.
type Held<K, E> = (K, Arc<E>, Arc<Runtime>);

/// The engine a model adapter built for a key (model ids, provider), with the runtime it runs on.
/// A different key builds a new engine; each run keeps the `ModelReleaser` timing the runtime.
pub struct ModelSlot<K, E> {
    slot: Mutex<Option<Held<K, E>>>,
    releaser: Arc<ModelReleaser>,
}

impl<K: PartialEq + Clone, E> ModelSlot<K, E> {
    pub fn new(releaser: Arc<ModelReleaser>) -> Self {
        ModelSlot { slot: Mutex::new(None), releaser }
    }

    /// The engine built for `key`, when that is the one held.
    pub fn get(&self, key: &K) -> Option<Arc<E>> {
        lock(&self.slot).as_ref().filter(|(k, _, _)| k == key).map(|(_, e, _)| e.clone())
    }

    /// The engine held, whatever its key.
    pub fn peek(&self) -> Option<Arc<E>> {
        lock(&self.slot).as_ref().map(|(_, e, _)| e.clone())
    }

    /// Holds a new engine and starts timing its runtime, so its sessions go even if the first run
    /// never returns (a dropped or panicking job).
    pub fn store(&self, key: K, engine: Arc<E>, runtime: Arc<Runtime>) {
        self.releaser.watch(&runtime);
        *lock(&self.slot) = Some((key, engine, runtime));
    }

    /// Call after a run: the release timer starts (again) from now.
    pub fn ran(&self) {
        let runtime = lock(&self.slot).as_ref().map(|(_, _, r)| r.clone());
        if let Some(runtime) = runtime {
            self.releaser.watch(&runtime);
        }
    }
}

/// Downloads ONNX Runtime when it is not there yet, reporting through `progress`.
pub async fn ensure_onnxruntime(data: &Path, progress: laqta_ml::Progress) -> Result<(), String> {
    let dir = models_dir(data);
    let ort = laqta_ml::install::ort_for_host().ok_or("no ONNX Runtime build for this platform")?;
    if laqta_ml::install::library_path(&dir, ort).exists() {
        return Ok(());
    }
    laqta_ml::install::install_onnxruntime(&dir, progress, Arc::new(AtomicBool::new(false))).await.map(|_| ()).map_err(|e| e.to_string())
}

/// Downloads smaller than this are not reported: they are over before a notice could be read.
const MIN_REPORTED_BYTES: u64 = 1_000_000;

/// Throttles the per-chunk progress of one download to a notice per quarter.
#[derive(Default)]
struct Quarters {
    last_done: u64,
    last_step: Option<u8>,
}

impl Quarters {
    /// The percentage (0, 25, 50, 75 or 100) to report when `done` of `total` bytes enters a quarter that has
    /// not been reported yet. A total that is unknown or small reports nothing, and a count that drops back
    /// means a new file started.
    fn advance(&mut self, done: u64, total: Option<u64>) -> Option<u8> {
        let total = total.filter(|t| *t >= MIN_REPORTED_BYTES)?;
        if done < self.last_done {
            *self = Quarters::default();
        }
        self.last_done = done;
        let step = u8::try_from(done.min(total).saturating_mul(4) / total).ok()? * 25;
        if self.last_step == Some(step) {
            return None;
        }
        self.last_step = Some(step);
        Some(step)
    }
}

/// A `Progress` for the first-use download of `what` (ADR 022: "with progress shown"). No window starts
/// these downloads, so the report goes through the `Notifier`: a toast under `title` at the start, at each
/// quarter and at the end.
pub fn toast_progress(notifier: &Arc<dyn Notifier>, title: &str, what: &str) -> laqta_ml::Progress {
    let (notifier, title, what) = (Arc::clone(notifier), title.to_owned(), what.to_owned());
    let quarters = Mutex::new(Quarters::default());
    Arc::new(move |done, total| {
        let Some(percent) = quarters.lock().ok().and_then(|mut q| q.advance(done, total)) else {
            return;
        };
        let body = match percent {
            0 => format!("Downloading {what}"),
            100 => format!("Downloaded {what}, checking it"),
            percent => format!("Downloading {what}: {percent} %"),
        };
        let options = ToastOptions { duration_ms: 1500, fade_ms: 300, ..ToastOptions::default() };
        if let Err(error) = notifier.toast(Toast { title: title.clone(), body, image: None, path: None, url: None, options }) {
            tracing::warn!(%error, "the download notice was not shown");
        }
    })
}

pub struct Models {
    pub data: PathBuf,
    pub options: SharedModelOptions,
}

impl Models {
    fn options(&self) -> ModelOptions {
        self.options.read().map(|o| o.clone()).unwrap_or_default()
    }
}

impl ModelService for Models {
    fn rows(&self) -> Vec<ModelRow> {
        let options = self.options();
        let rt = runtime(&self.data, &options);
        let mut rows: Vec<ModelRow> = [Task::OcrDet, Task::OcrRec, Task::Background, Task::Inpaint]
            .into_iter()
            .flat_map(|t| catalog().models_for(t))
            .map(|e| ModelRow {
                id: e.id.clone(),
                task: e.task.as_str().to_owned(),
                name: e.name.clone(),
                description: e.description.clone(),
                size_bytes: e.size_bytes,
                licence: e.licence.clone(),
                commercial: e.commercial,
                installed: rt.as_ref().is_some_and(|r| r.is_installed(e)),
                custom: false,
                available: !e.is_pending(),
            })
            .collect();
        let dir = models_dir(&self.data);
        rows.extend(options.custom.iter().map(|c| ModelRow {
            id: c.id.clone(),
            task: c.task.clone(),
            name: c.name.clone(),
            description: format!("Custom model ({})", c.family),
            size_bytes: std::fs::metadata(dir.join(format!("{}.onnx", c.id))).map(|m| m.len()).unwrap_or(0),
            licence: String::new(),
            commercial: true,
            installed: dir.join(format!("{}.onnx", c.id)).is_file(),
            custom: true,
            available: true,
        }));
        rows
    }

    fn families(&self) -> Vec<(String, String)> {
        Family::ALL.iter().map(|f| (f.task().as_str().to_owned(), f.name().to_owned())).collect()
    }

    fn install(&self, id: String, progress: ModelProgress) -> BoxFuture<'static, Result<(), String>> {
        let data = self.data.clone();
        let options = self.options();
        Box::pin(async move {
            let entry = catalog().model(&id).ok_or_else(|| format!("no model {id}"))?;
            let rt = runtime(&data, &options).ok_or("no ONNX Runtime build for this platform")?;
            let p: laqta_ml::Progress = Arc::new(move |a, b| progress(a, b));
            rt.install(entry, p, Arc::new(AtomicBool::new(false))).await.map_err(|e| e.to_string())
        })
    }

    fn remove(&self, id: &str) -> Result<(), String> {
        let dir = models_dir(&self.data);
        for f in [format!("{id}.onnx"), format!("{id}.dict.txt")] {
            match std::fs::remove_file(dir.join(&f)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("{f}: {e}")),
            }
        }
        Ok(())
    }

    fn add_custom(&self, path: PathBuf, family: String, name: String) -> BoxFuture<'static, Result<CustomModel, String>> {
        let data = self.data.clone();
        let options = self.options();
        Box::pin(async move {
            let fam = Family::parse(&family).ok_or_else(|| format!("unknown family {family}"))?;
            // The model manager has no channel for this download (`add_custom` takes no progress).
            ensure_onnxruntime(&data, Arc::new(|_, _| {})).await?;
            let rt = runtime(&data, &options).ok_or("no ONNX Runtime build for this platform")?;
            rt.ensure_ort().map_err(|e| e.to_string())?;
            laqta_ml::validate_custom(&path, &family).map_err(|e| e.to_string())?;
            let slug: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
            let id = format!("custom-{}-{}", fam.name(), slug.trim_matches('-'));
            let dir = models_dir(&data);
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            std::fs::copy(&path, dir.join(format!("{id}.onnx"))).map_err(|e| e.to_string())?;
            Ok(CustomModel { id, name, task: fam.task().as_str().to_owned(), family })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use laqta_ports::PortError;

    const MB: u64 = 1_000_000;

    /// A notifier that keeps the toasts it was given.
    #[derive(Default)]
    struct Recorder(Mutex<Vec<Toast>>);

    impl Notifier for Recorder {
        fn toast(&self, toast: Toast) -> Result<(), PortError> {
            self.0.lock().expect("lock").push(toast);
            Ok(())
        }
    }

    fn bodies(recorder: &Recorder) -> Vec<String> {
        recorder.0.lock().expect("lock").iter().map(|t| t.body.clone()).collect()
    }

    #[test]
    fn a_download_reports_each_quarter_once() {
        let mut quarters = Quarters::default();
        let total = Some(100 * MB);
        let mut reported = Vec::new();
        for done in (1..=100).map(|i| i * MB) {
            reported.extend(quarters.advance(done, total));
            // A repeated chunk size inside a quarter reports nothing more.
            assert_eq!(quarters.advance(done, total), None);
        }
        assert_eq!(reported, [0, 25, 50, 75, 100]);
    }

    #[test]
    fn a_small_or_unsized_download_reports_nothing() {
        let mut quarters = Quarters::default();
        assert_eq!(quarters.advance(74_012, Some(74_012)), None, "a dictionary is over at once");
        assert_eq!(quarters.advance(10 * MB, None), None, "no total, no percentage");
    }

    #[test]
    fn a_second_file_starts_again_from_zero() {
        let mut quarters = Quarters::default();
        assert_eq!(quarters.advance(8 * MB, Some(8 * MB)), Some(100));
        assert_eq!(quarters.advance(MB / 2, Some(8 * MB)), Some(0), "the count dropped, so this is a new file");
    }

    #[test]
    fn the_progress_posts_a_toast_per_quarter_under_the_title() {
        let recorder = Arc::new(Recorder::default());
        let notifier: Arc<dyn Notifier> = recorder.clone();
        let progress = toast_progress(&notifier, "Background removal", "BiRefNet lite, fp16");
        for done in (1..=115).map(|i| i * MB) {
            progress(done, Some(115 * MB));
        }
        assert_eq!(
            bodies(&recorder),
            [
                "Downloading BiRefNet lite, fp16",
                "Downloading BiRefNet lite, fp16: 25 %",
                "Downloading BiRefNet lite, fp16: 50 %",
                "Downloading BiRefNet lite, fp16: 75 %",
                "Downloaded BiRefNet lite, fp16, checking it",
            ]
        );
        assert!(recorder.0.lock().expect("lock").iter().all(|t| t.title == "Background removal"));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod release_tests {
    use super::*;
    use laqta_ports::fakes::{InlineSpawner, ManualClock};

    fn rig() -> (Arc<InlineSpawner>, Arc<ModelReleaser>) {
        let spawner = Arc::new(InlineSpawner::default());
        let clock = Arc::new(ManualClock::new(jiff::civil::date(2026, 11, 10).at(9, 0, 0, 0)));
        (spawner.clone(), ModelReleaser::new(clock, spawner))
    }

    fn runtime() -> Arc<Runtime> {
        Arc::new(Runtime::new("/nonexistent/libonnxruntime.so", "/nonexistent-models", ExecutionProvider::Cpu))
    }

    #[test]
    fn one_timer_however_often_a_model_runs() {
        let (spawner, releaser) = rig();
        let rt = runtime();
        releaser.watch(&rt);
        releaser.watch(&rt);
        assert_eq!(spawner.take_spawned().len(), 1);
        assert_eq!(lock(&releaser.state.runtimes).len(), 1, "a runtime is listed once");
    }

    #[test]
    fn the_timer_disarms_when_nothing_is_loaded() {
        let (spawner, releaser) = rig();
        releaser.watch(&runtime());
        let timer = spawner.take_spawned().pop().unwrap();
        futures_lite::future::block_on(timer);
        assert_eq!(*spawner.blocking_calls.lock().unwrap(), 1, "release_idle ran off the async worker");
        assert!(!*lock(&releaser.state.armed));
        // The next use arms a new one.
        releaser.watch(&runtime());
        assert_eq!(spawner.take_spawned().len(), 1);
    }

    #[test]
    fn a_slot_hands_out_only_the_engine_for_its_key() {
        let (_, releaser) = rig();
        let slot: ModelSlot<(String, ModelProvider), u8> = ModelSlot::new(releaser);
        let key = ("a".to_owned(), ModelProvider::Cpu);
        slot.store(key.clone(), Arc::new(1), runtime());
        assert_eq!(slot.get(&key).as_deref(), Some(&1));
        assert!(slot.get(&("a".to_owned(), ModelProvider::Gpu)).is_none(), "a provider change builds a new engine");
        assert_eq!(slot.peek().as_deref(), Some(&1));
    }
}
