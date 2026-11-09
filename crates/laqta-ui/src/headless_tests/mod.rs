//! Real views on GPUI's test platform: no display, no desktop automation (docs/plan/UI-HARNESS.md
//! T6). `services()` builds a `UiServices` from the port fakes; `QueuedSpawner` holds compute work
//! until the test runs it, which models a render slower than the input stream.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod editor;
mod overlay;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use futures_channel::oneshot;
use gpui::{AppContext as _, TestAppContext};
use laqta_app::{App, Ports};
use laqta_domain::Settings;
use laqta_ports::fakes::{ClipboardBehaviour, FakeClipboard, FakeScreen, FakeShell, InlineSpawner, ManualClock};
use laqta_ports::{BoxFuture, PortError, Spawner};

use crate::presenters::task_list::TaskList;
use crate::runtime::{ModelProgress, ModelRow, ModelService, UiServices, UiState};

type Job = (Box<dyn FnOnce() + Send>, oneshot::Sender<()>);

/// A `Spawner` whose compute work waits in a queue until the test runs it. Blocking work and
/// spawned futures run inline, as `InlineSpawner` does.
#[derive(Clone, Default)]
pub(crate) struct QueuedSpawner {
    queue: Arc<Mutex<Vec<Job>>>,
    calls: Arc<AtomicUsize>,
    inline: Arc<InlineSpawner>,
}

impl QueuedSpawner {
    /// `run_compute` calls so far.
    pub(crate) fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    pub(crate) fn reset(&self) {
        self.calls.store(0, Ordering::SeqCst);
    }

    /// Runs every queued job and resolves its future. Returns how many ran.
    ///
    /// Jobs run on the test thread, made a rayon worker (imaging kernels assert a pool): GPUI's
    /// test scheduler rejects a task woken from any other thread.
    pub(crate) fn run_all(&self) -> usize {
        thread_local! {
            static POOL: rayon::ThreadPool =
                rayon::ThreadPoolBuilder::new().num_threads(1).use_current_thread().build().unwrap();
        }
        let jobs: Vec<Job> = std::mem::take(&mut *self.queue.lock().unwrap());
        let n = jobs.len();
        POOL.with(|pool| {
            for (job, done) in jobs {
                pool.install(job);
                let _ = done.send(());
            }
        });
        n
    }
}

impl Spawner for QueuedSpawner {
    fn spawn(&self, task: BoxFuture<'static, ()>) {
        self.inline.spawn(task);
    }
    fn run_blocking(&self, work: Box<dyn FnOnce() + Send>) -> BoxFuture<'static, ()> {
        self.inline.run_blocking(work)
    }
    fn run_compute(&self, work: Box<dyn FnOnce() + Send>) -> BoxFuture<'static, ()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.queue.lock().unwrap().push((work, tx));
        Box::pin(async move {
            let _ = rx.await;
        })
    }
}

impl std::fmt::Debug for QueuedSpawner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueuedSpawner").field("calls", &self.calls()).finish_non_exhaustive()
    }
}

struct NoModels;
impl ModelService for NoModels {
    fn rows(&self) -> Vec<ModelRow> {
        Vec::new()
    }
    fn families(&self) -> Vec<(String, String)> {
        Vec::new()
    }
    fn install(&self, _: String, _: ModelProgress) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async { Err("no models in tests".into()) })
    }
    fn remove(&self, _: &str) -> Result<(), String> {
        Err("no models in tests".into())
    }
    fn add_custom(&self, _: PathBuf, _: String, _: String) -> BoxFuture<'static, Result<laqta_domain::settings::CustomModel, String>> {
        Box::pin(async { Err("no models in tests".into()) })
    }
}

struct NoShell;
impl laqta_ports::Autostart for NoShell {
    fn is_enabled(&self) -> Result<bool, PortError> {
        Ok(false)
    }
    fn set_enabled(&self, _: bool, _: &Path) -> Result<(), PortError> {
        Err(PortError::Unsupported("tests".into()))
    }
}
impl laqta_ports::ShellIntegration for NoShell {
    fn is_installed(&self) -> Result<bool, PortError> {
        Ok(false)
    }
    fn install(&self, _: &Path) -> Result<(), PortError> {
        Err(PortError::Unsupported("tests".into()))
    }
    fn uninstall(&self) -> Result<(), PortError> {
        Err(PortError::Unsupported("tests".into()))
    }
    fn install_native_messaging(&self, _: &Path) -> Result<(), PortError> {
        Err(PortError::Unsupported("tests".into()))
    }
}

/// `UiServices` built from the port fakes, with `spawner` as the compute pool.
pub(crate) fn services(spawner: QueuedSpawner, dir: &Path) -> UiServices {
    let spawner: Arc<dyn Spawner> = Arc::new(spawner);
    let shell = Arc::new(FakeShell::default());
    let clock = Arc::new(ManualClock::new(jiff::civil::date(2026, 11, 9).at(12, 0, 0, 0)));
    let ports = Ports {
        screen: Arc::new(FakeScreen::new(Vec::new())),
        clipboard: Arc::new(FakeClipboard::new(ClipboardBehaviour::Succeed)),
        shell: shell.clone(),
        clock: clock.clone(),
        spawner: spawner.clone(),
        overlay: Arc::new(laqta_ports::fakes::FakeOverlay::default()),
        tools: Arc::new(laqta_ports::fakes::FakeToolHost::default()),
        prompt: Arc::new(laqta_ports::fakes::FakePrompt::default()),
        editor: Arc::new(laqta_ports::fakes::FakeEditorHost::default()),
        media: Arc::new(laqta_ports::fakes::FakeMedia::default()),
        stream: Arc::new(laqta_ports::fakes::FakeScreenStream::default()),
        ocr: Arc::new(laqta_ports::fakes::FakeOcr::default()),
        window_ops: Arc::new(laqta_ports::fakes::FakeWindowOps::default()),
        input: Arc::new(laqta_ports::fakes::FakeInputSynth::default()),
        highlighter: Arc::new(laqta_ports::fakes::FakeHighlighter::default()),
        background: Arc::new(laqta_ports::fakes::FakeBackgroundRemover::default()),
        analyzer: Arc::new(laqta_ports::fakes::FakeAnalyzer::default()),
        inpainter: Arc::new(laqta_ports::fakes::FakeInpainter),
        watcher: Arc::new(laqta_ports::fakes::FakeFileWatcher::default()),
        uploaders: Arc::new(laqta_ports::fakes::FakeUploaders::default()),
        hover: Arc::new(laqta_ports::fakes::FakeHoverSource::new(Vec::new())),
        notifier: Arc::new(laqta_ports::fakes::FakeNotifier::default()),
        rng: Arc::new(laqta_ports::fakes::CountingRng::default()),
        host: laqta_ports::HostInfo::default(),
        codecs: laqta_imaging::Codecs::builtin(),
    };
    let settings = Settings::default_for(dir.to_path_buf());
    UiServices {
        hotkey_report: Arc::new(crate::runtime::HotkeyReport::default),
        app: App::new(ports, settings),
        history: None,
        shell,
        clock,
        host: laqta_ports::HostInfo::default(),
        dispatch: Arc::new(|_| {}),
        save_settings: Arc::new(|_| Ok(())),
        data_dir: dir.to_path_buf(),
        uploaders: Arc::new(laqta_ports::fakes::FakeUploaders::default()),
        secrets: Arc::new(laqta_ports::fakes::FakeSecretStore::default()),
        add_custom_uploader: Arc::new(|_| Err("tests".into())),
        import_sharex: Arc::new(|_| Err("tests".into())),
        sxie_read: Arc::new(|_| Err("tests".into())),
        sxie_write: Arc::new(|_| Err("tests".into())),
        remote_storages: Arc::new(Vec::new),
        models: Arc::new(NoModels),
        check_update: Arc::new(|| Box::pin(async { Err("tests".into()) })),
        install_update: Arc::new(|| Box::pin(async { Err("tests".into()) })),
        autostart: Arc::new(NoShell),
        integration: Arc::new(NoShell),
        fonts: laqta_editor::Fonts::empty(),
        spawner,
        codecs: laqta_imaging::Codecs::builtin(),
    }
}

/// What `runtime::run` sets up, minus the event loop and the platform.
pub(crate) fn install(cx: &mut TestAppContext, services: UiServices) {
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::theme::apply(services.app.settings().theme, cx);
        let tasks = cx.new(|_| TaskList::default());
        cx.set_global(UiState {
            services,
            tasks,
            overlay: crate::overlay::OverlayWindows::default(),
            main: None,
            settings: None,
            history: None,
            tray_popup: None,
            actions_toolbar: None,
            recording: Default::default(),
        });
    });
}
