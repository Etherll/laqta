//! The bridge between the application (any thread) and GPUI (the main thread).
//!
//! `UiHandle` implements the `Overlay`, `Notifier` and `ToolHost` ports by posting commands over a
//! channel. `run` owns the GPUI application loop and executes them.

use std::path::PathBuf;
use std::sync::Arc;

use futures_channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures_lite::StreamExt;
use gpui::{App, AppContext as _, AsyncApp, Entity};
use laqta_app::App as LaqtaApp;
use laqta_domain::{Action, Frame, Settings};
use laqta_library::History;
use laqta_ports::{
    Clock, HostInfo, InputCallback, Notifier, Overlay, PortError, RegionView, Shell, TaskEvent, Toast, ToolHost, ToolWindow,
};

use crate::presenters::task_list::TaskList;
use crate::{history_window, main_window, overlay, settings_window, toast, tray_popup};

pub(crate) enum UiCommand {
    ShowOverlay { frames: Vec<Frame>, view: RegionView, on_input: InputCallback },
    UpdateOverlay(RegionView),
    CloseOverlay,
    Toast(Toast),
    Open(ToolWindow),
    Pin(laqta_domain::Bitmap),
    ClosePins,
    ShowText { title: String, text: String },
    Edit { image: laqta_domain::Bitmap, reply: futures_channel::oneshot::Sender<Result<laqta_domain::Bitmap, PortError>> },
    Prompt { title: String, kind: crate::prompt_window::PromptKind, reply: crate::prompt_window::Reply },
    Quit,
}

/// A cheap, thread-safe handle to the UI. Clone it into every adapter that needs the UI.
#[derive(Clone, Debug)]
pub struct UiHandle {
    tx: UnboundedSender<UiCommand>,
}

/// The receiving end, consumed by [`run`].
#[derive(Debug)]
pub struct UiReceiver(UnboundedReceiver<UiCommand>);

impl UiHandle {
    pub fn channel() -> (UiHandle, UiReceiver) {
        let (tx, rx) = unbounded();
        (UiHandle { tx }, UiReceiver(rx))
    }

    fn send(&self, cmd: UiCommand) -> Result<(), PortError> {
        self.tx.unbounded_send(cmd).map_err(|_| PortError::Unsupported("the UI is not running".into()))
    }

    /// Asks the GPUI loop to quit.
    pub fn quit(&self) {
        let _ = self.send(UiCommand::Quit);
    }
}

impl Overlay for UiHandle {
    fn show(&self, frames: Vec<Frame>, view: RegionView, on_input: InputCallback) -> Result<(), PortError> {
        self.send(UiCommand::ShowOverlay { frames, view, on_input })
    }
    fn update(&self, view: RegionView) {
        let _ = self.send(UiCommand::UpdateOverlay(view));
    }
    fn close(&self) {
        let _ = self.send(UiCommand::CloseOverlay);
    }
}

impl Notifier for UiHandle {
    fn toast(&self, toast: Toast) -> Result<(), PortError> {
        self.send(UiCommand::Toast(toast))
    }
}

impl UiHandle {
    fn ask(
        &self,
        title: &str,
        kind: crate::prompt_window::PromptKind,
    ) -> futures_channel::oneshot::Receiver<Result<crate::prompt_window::Answer, PortError>> {
        let (tx, rx) = futures_channel::oneshot::channel();
        if let Err(e) = self.tx.unbounded_send(UiCommand::Prompt { title: title.to_string(), kind, reply: tx })
            && let UiCommand::Prompt { reply, .. } = e.into_inner()
        {
            let _ = reply.send(Err(PortError::Unsupported("the UI is not running".into())));
        }
        rx
    }
}

impl laqta_ports::Prompt for UiHandle {
    fn ask_text(&self, title: &str, default: &str) -> laqta_ports::BoxFuture<'_, Result<String, PortError>> {
        use crate::prompt_window::{Answer, PromptKind};
        let rx = self.ask(title, PromptKind::Text { default: default.to_string() });
        Box::pin(async move {
            match rx.await {
                Ok(Ok(Answer::Text(t))) => Ok(t),
                Ok(Err(e)) => Err(e),
                _ => Err(PortError::Cancelled),
            }
        })
    }
    fn choose(&self, title: &str, options: &[String]) -> laqta_ports::BoxFuture<'_, Result<usize, PortError>> {
        use crate::prompt_window::{Answer, PromptKind};
        let rx = self.ask(title, PromptKind::Choose { options: options.to_vec() });
        Box::pin(async move {
            match rx.await {
                Ok(Ok(Answer::Index(i))) => Ok(i),
                Ok(Err(e)) => Err(e),
                _ => Err(PortError::Cancelled),
            }
        })
    }
    fn show_text(&self, title: &str, text: &str) -> laqta_ports::BoxFuture<'_, Result<(), PortError>> {
        use crate::prompt_window::PromptKind;
        let rx = self.ask(title, PromptKind::Show { text: text.to_string() });
        Box::pin(async move {
            match rx.await {
                Ok(Ok(_)) => Ok(()),
                Ok(Err(e)) => Err(e),
                Err(_) => Err(PortError::Cancelled),
            }
        })
    }
    fn pick_files(&self, title: &str) -> laqta_ports::BoxFuture<'_, Result<Vec<PathBuf>, PortError>> {
        let title = title.to_string();
        Box::pin(async move {
            match rfd::AsyncFileDialog::new().set_title(title).pick_files().await {
                Some(files) => Ok(files.iter().map(|f| f.path().to_path_buf()).collect()),
                None => Err(PortError::Cancelled),
            }
        })
    }
    fn pick_folder(&self, title: &str) -> laqta_ports::BoxFuture<'_, Result<PathBuf, PortError>> {
        let title = title.to_string();
        Box::pin(async move {
            match rfd::AsyncFileDialog::new().set_title(title).pick_folder().await {
                Some(f) => Ok(f.path().to_path_buf()),
                None => Err(PortError::Cancelled),
            }
        })
    }
    fn save_path(&self, suggested: &std::path::Path) -> laqta_ports::BoxFuture<'_, Result<PathBuf, PortError>> {
        let mut d = rfd::AsyncFileDialog::new();
        if let Some(dir) = suggested.parent() {
            d = d.set_directory(dir);
        }
        if let Some(name) = suggested.file_name() {
            d = d.set_file_name(name.to_string_lossy());
        }
        Box::pin(async move {
            match d.save_file().await {
                Some(f) => Ok(f.path().to_path_buf()),
                None => Err(PortError::Cancelled),
            }
        })
    }
    fn after_capture(
        &self,
        image: &laqta_domain::Bitmap,
        after_capture: u32,
        after_upload: u32,
    ) -> laqta_ports::BoxFuture<'_, Result<(u32, u32), PortError>> {
        use crate::prompt_window::{Answer, PromptKind};
        // GPUI scales the full image; `thumbnail` would need the compute pool.
        let rx =
            self.ask("After capture", PromptKind::AfterCapture { image: crate::toast::render_image(image), after_capture, after_upload });
        Box::pin(async move {
            match rx.await {
                Ok(Ok(Answer::Flags(a, b))) => Ok((a, b)),
                Ok(Err(e)) => Err(e),
                _ => Err(PortError::Cancelled),
            }
        })
    }
    fn before_upload(
        &self,
        file_name: &str,
        destinations: &[(String, String)],
        current: &str,
    ) -> laqta_ports::BoxFuture<'_, Result<String, PortError>> {
        use crate::prompt_window::{Answer, PromptKind};
        let rx = self.ask(
            "Before upload",
            PromptKind::BeforeUpload {
                file_name: file_name.to_string(),
                destinations: destinations.to_vec(),
                current: current.to_string(),
            },
        );
        Box::pin(async move {
            match rx.await {
                Ok(Ok(Answer::Text(id))) => Ok(id),
                Ok(Err(e)) => Err(e),
                _ => Err(PortError::Cancelled),
            }
        })
    }
    fn confirm(&self, title: &str, text: &str) -> laqta_ports::BoxFuture<'_, Result<bool, PortError>> {
        use crate::prompt_window::{Answer, PromptKind};
        let rx = self.ask(title, PromptKind::Confirm { text: text.to_string() });
        Box::pin(async move {
            match rx.await {
                Ok(Ok(Answer::Bool(b))) => Ok(b),
                Ok(Err(e)) => Err(e),
                _ => Err(PortError::Cancelled),
            }
        })
    }
}

impl laqta_ports::EditorHost for UiHandle {
    fn edit(&self, image: laqta_domain::Bitmap) -> laqta_ports::BoxFuture<'_, Result<laqta_domain::Bitmap, PortError>> {
        let (tx, rx) = futures_channel::oneshot::channel();
        let sent = self.tx.unbounded_send(UiCommand::Edit { image, reply: tx });
        Box::pin(async move {
            if sent.is_err() {
                return Err(PortError::Unsupported("the UI is not running".into()));
            }
            rx.await.unwrap_or(Err(PortError::Cancelled))
        })
    }
}

impl ToolHost for UiHandle {
    fn open(&self, window: ToolWindow) -> Result<(), PortError> {
        self.send(UiCommand::Open(window))
    }
    fn pin(&self, image: laqta_domain::Bitmap) -> Result<(), PortError> {
        self.send(UiCommand::Pin(image))
    }
    fn close_pins(&self) -> Result<(), PortError> {
        self.send(UiCommand::ClosePins)
    }
    fn show_text(&self, title: &str, text: &str) -> Result<(), PortError> {
        self.send(UiCommand::ShowText { title: title.to_owned(), text: text.to_owned() })
    }
}

/// Checks for an update: the message to show and whether `install_update` can apply it here.
pub type UpdateCheck = Arc<dyn Fn() -> laqta_ports::BoxFuture<'static, Result<(String, bool), String>> + Send + Sync>;
pub type UpdateInstall = Arc<dyn Fn() -> laqta_ports::BoxFuture<'static, Result<(), String>> + Send + Sync>;
/// One model in the model manager (catalog or custom).
#[derive(Clone, Debug)]
pub struct ModelRow {
    pub id: String,
    /// The catalog task name (`ocr-det`, `ocr-rec`, `background`).
    pub task: String,
    pub name: String,
    pub description: String,
    pub size_bytes: u64,
    pub licence: String,
    pub commercial: bool,
    pub installed: bool,
    pub custom: bool,
    /// False while the catalog has no pinned hash for it.
    pub available: bool,
}

pub type ModelProgress = Arc<dyn Fn(u64, Option<u64>) + Send + Sync>;

/// The model manager's view of `laqta-ml` (ADR 023), built by `main`.
pub trait ModelService: Send + Sync {
    fn rows(&self) -> Vec<ModelRow>;
    /// (task, family) pairs a custom model can use.
    fn families(&self) -> Vec<(String, String)>;
    fn install(&self, id: String, progress: ModelProgress) -> laqta_ports::BoxFuture<'static, Result<(), String>>;
    fn remove(&self, id: &str) -> Result<(), String>;
    /// Checks the file against the family and copies it into the models folder.
    fn add_custom(&self, path: PathBuf, family: String, name: String) -> laqta_ports::BoxFuture<'static, Result<laqta_domain::settings::CustomModel, String>>;
}

pub type SxieRead = Arc<dyn Fn(&[u8]) -> Result<laqta_domain::effects::Preset, String> + Send + Sync>;
pub type SxieWrite = Arc<dyn Fn(&laqta_domain::effects::Preset) -> Result<Vec<u8>, String> + Send + Sync>;

/// What the views need from the rest of the program. Built by `main`.
#[derive(Clone)]
pub struct UiServices {
    pub app: Arc<LaqtaApp>,
    pub history: Option<Arc<History>>,
    pub shell: Arc<dyn Shell>,
    pub clock: Arc<dyn Clock>,
    pub host: HostInfo,
    /// Runs an action through the dispatcher on the application executor.
    pub dispatch: Arc<dyn Fn(Action) + Send + Sync>,
    /// Persists new settings and applies them (store, `App::set_settings`, hotkeys).
    pub save_settings: Arc<dyn Fn(Settings) -> Result<(), String> + Send + Sync>,
    /// The data folder, shown in settings.
    pub data_dir: PathBuf,
    pub uploaders: Arc<dyn laqta_ports::Uploaders>,
    pub secrets: Arc<dyn laqta_ports::SecretStore>,
    /// Adds a custom uploader (`.sxcu` JSON): saves it and registers it; returns its id.
    pub add_custom_uploader: Arc<dyn Fn(String) -> Result<String, String> + Send + Sync>,
    /// Imports a ShareX folder or `.sxb` backup (settings, workflows, uploaders, secrets, history) and
    /// applies it; returns a summary.
    pub import_sharex: Arc<dyn Fn(PathBuf) -> Result<String, String> + Send + Sync>,
    /// Reads and writes ShareX `.sxie` effect packages (`laqta-import`).
    pub sxie_read: SxieRead,
    pub sxie_write: SxieWrite,
    /// The model manager (ADR 023).
    pub models: Arc<dyn ModelService>,
    /// Update check and install (M6 D12).
    pub check_update: UpdateCheck,
    pub install_update: UpdateInstall,
    /// Run at login (M6 D10).
    pub autostart: Arc<dyn laqta_ports::Autostart>,
    /// "Upload with Laqta" in file managers, Send To, and the browser native-messaging host (M6 D11).
    pub integration: Arc<dyn laqta_ports::ShellIntegration>,
    /// The font handle shared with the Editor engine (ADR 024).
    pub fonts: laqta_editor::Fonts,
    /// The compute pool and blocking pool (`run_compute` for editor operations that need it).
    pub spawner: Arc<dyn laqta_ports::Spawner>,
    pub codecs: laqta_imaging::Codecs,
}

impl std::fmt::Debug for UiServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UiServices").finish_non_exhaustive()
    }
}

/// UI state that lives on the GPUI thread.
pub(crate) struct UiState {
    pub services: UiServices,
    pub tasks: Entity<TaskList>,
    pub overlay: overlay::OverlayWindows,
    pub main: Option<gpui::AnyWindowHandle>,
    pub settings: Option<gpui::AnyWindowHandle>,
    pub history: Option<gpui::AnyWindowHandle>,
    pub tray_popup: Option<gpui::AnyWindowHandle>,
    pub actions_toolbar: Option<gpui::AnyWindowHandle>,
    pub recording: crate::recording_bar::RecordingWindows,
}

impl gpui::Global for UiState {}

/// Runs GPUI on the current (main) thread until a `Quit` command arrives. Opens no window by itself.
pub fn run(services: UiServices, rx: UiReceiver) {
    // Laqta lives in the tray: closing the last window must not quit it.
    gpui_platform::application().with_quit_mode(gpui::QuitMode::Explicit).run(move |cx: &mut App| {
        gpui_component::init(cx);
        crate::theme::apply(services.app.settings().theme, cx);
        let tasks = cx.new(|_| TaskList::default());
        cx.set_global(UiState {
            services: services.clone(),
            tasks: tasks.clone(),
            overlay: overlay::OverlayWindows::default(),
            main: None,
            settings: None,
            history: None,
            tray_popup: None,
            actions_toolbar: None,
            recording: Default::default(),
        });

        // Task events feed the task list; this task sleeps until an event arrives.
        let mut events = services.app.events();
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Ok(event) = events.recv().await {
                let event: TaskEvent = event;
                {
                    let e = event.clone();
                    cx.update(|cx| crate::recording_bar::on_event(&e, cx));
                }
                if let TaskEvent::ShowUrl { url, qr, .. } = &event {
                    let (url, qr) = (url.clone(), *qr);
                    cx.update(|cx| crate::url_window::open(url, qr, cx));
                }
                cx.update(|cx| {
                    tasks.update(cx, |list, cx| {
                        list.apply(&event);
                        cx.notify();
                    })
                });
            }
        })
        .detach();

        let mut rx = rx.0;
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Some(cmd) = rx.next().await {
                let quit = matches!(cmd, UiCommand::Quit);
                cx.update(|cx| handle(cmd, cx));
                if quit {
                    break;
                }
            }
        })
        .detach();
    });
}

fn handle(cmd: UiCommand, cx: &mut App) {
    match cmd {
        UiCommand::ShowOverlay { frames, view, on_input } => overlay::show(frames, view, on_input, cx),
        UiCommand::UpdateOverlay(view) => overlay::update(view, cx),
        UiCommand::CloseOverlay => overlay::close(cx),
        UiCommand::Toast(t) => toast::show(t, cx),
        UiCommand::Open(ToolWindow::Main) => main_window::open(cx),
        UiCommand::Open(ToolWindow::Settings) => settings_window::open(cx),
        UiCommand::Open(ToolWindow::History) => history_window::open(false, cx),
        UiCommand::Open(ToolWindow::ImageHistory) => history_window::open(true, cx),
        UiCommand::Open(ToolWindow::TrayMenu) => tray_popup::toggle(cx),
        UiCommand::Open(ToolWindow::ActionsToolbar) => crate::actions_toolbar::toggle(cx),
        UiCommand::Prompt { title, kind, reply } => crate::prompt_window::open(title, kind, reply, cx),
        UiCommand::Edit { image, reply } => crate::editor_window::open(Some(image), Some(reply), None, cx),
        UiCommand::Open(ToolWindow::Editor(path)) => crate::editor_window::open_file(path, cx),
        UiCommand::Open(ToolWindow::Effects(path)) => crate::effects_window::open(path, cx),
        UiCommand::Open(ToolWindow::Tool(kind)) => crate::tools_window::open(kind, cx),
        UiCommand::Pin(image) => crate::pin_window::pin(image, cx),
        UiCommand::ClosePins => crate::pin_window::close_all(cx),
        UiCommand::ShowText { title, text } => crate::pin_window::show_text(title, text, cx),
        UiCommand::Open(other) => tracing::warn!(?other, "no window for this tool yet"),
        UiCommand::Quit => {
            overlay::close(cx);
            cx.quit();
        }
    }
}

/// Focuses an existing window if its handle is still alive. Returns false if it was closed.
pub(crate) fn activate(handle: Option<gpui::AnyWindowHandle>, cx: &mut App) -> bool {
    match handle {
        Some(h) => h.update(cx, |_, window, _| window.activate_window()).is_ok(),
        None => false,
    }
}
