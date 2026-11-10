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
use laqta_domain::{Action, Bitmap, Frame, Settings};
use laqta_library::History;
use laqta_ports::{
    Clock, HostInfo, InputCallback, Notifier, Overlay, PortError, RegionView, Shell, TaskEvent, Toast, ToolHost, ToolWindow,
};

use crate::presenters::task_list::TaskList;
use crate::{history_window, main_window, overlay, settings_window, toast, tray_popup};

pub(crate) enum UiCommand {
    ShowOverlay {
        frames: Vec<Frame>,
        images: Vec<Option<std::sync::Arc<gpui::RenderImage>>>,
        view: RegionView,
        on_input: InputCallback,
    },
    UpdateOverlay(RegionView),
    CloseOverlay,
    Toast(Toast),
    Open(ToolWindow),
    Pin(laqta_domain::Bitmap),
    PhotoEditor(laqta_domain::Bitmap),
    ClosePins,
    ShowText {
        title: String,
        text: String,
    },
    /// OCR: the text beside the image it came from.
    ShowOcr {
        image: Bitmap,
        text: String,
    },
    /// A colour picked from the screen.
    ShowColor(laqta_domain::color::Rgba),
    Edit {
        image: laqta_domain::Bitmap,
        reply: futures_channel::oneshot::Sender<Result<laqta_domain::Bitmap, PortError>>,
    },
    Prompt {
        title: String,
        kind: crate::prompt_window::PromptKind,
        reply: crate::prompt_window::Reply,
    },
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
        let images = overlay::render_images(&frames);
        self.send(UiCommand::ShowOverlay { frames, images, view, on_input })
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
    fn pick_file(&self, title: &str, extensions: &[&str]) -> laqta_ports::BoxFuture<'_, Result<PathBuf, PortError>> {
        let mut d = rfd::AsyncFileDialog::new().set_title(title);
        if !extensions.is_empty() {
            d = d.add_filter(extensions.join(", "), extensions);
        }
        Box::pin(async move {
            match d.pick_file().await {
                Some(f) => Ok(f.path().to_path_buf()),
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
    fn photo_editor(&self, image: laqta_domain::Bitmap) -> Result<(), PortError> {
        self.send(UiCommand::PhotoEditor(image))
    }
    fn close_pins(&self) -> Result<(), PortError> {
        self.send(UiCommand::ClosePins)
    }
    fn show_text(&self, title: &str, text: &str) -> Result<(), PortError> {
        self.send(UiCommand::ShowText { title: title.to_owned(), text: text.to_owned() })
    }
    fn show_color(&self, color: laqta_domain::color::Rgba) -> Result<(), PortError> {
        self.send(UiCommand::ShowColor(color))
    }
    fn show_ocr(&self, image: Bitmap, result: laqta_ports::OcrResult) -> Result<(), PortError> {
        self.send(UiCommand::ShowOcr { image, text: result.text() })
    }
}

/// What `UiServices::native_window` does to a window. The first three are Windows behaviour and
/// the last is macOS behaviour; a platform answers false to what it does not need.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeWindow {
    /// A borderless popup keeps only what it paints: no outline or shadow drawn by the OS.
    PlainPopup,
    /// The window becomes the foreground window and gets the keyboard, which the OS withholds
    /// from the region overlay once Laqta's main window has minimized.
    BringToFront,
    /// The window leaves the screen at once and ends up minimized, and the call returns when the
    /// desktop has been drawn without it. A plain minimize is animated, and shows in a capture.
    HideForCapture,
    /// The window covers its whole screen, menu bar and Dock included. macOS keeps a new window
    /// below the menu bar, which would draw the region overlay's frozen screen lower than the
    /// real one.
    CoverScreen,
}

/// `UiServices::native_window`: true when the platform did what was asked.
pub type NativeWindowHook = Arc<dyn Fn(&gpui::Window, NativeWindow) -> bool + Send + Sync>;

/// `UiServices::logical_rect`: a rect in the ports' physical pixels as `[x, y, width, height]` in
/// the desktop's logical pixels. Windows and X11 need none: there a rect divided by its display's
/// scale is its logical rect. macOS has no physical desktop, so its adapter lays one out and is the
/// only one that can map back (ADR 030).
pub type LogicalRectHook = Arc<dyn Fn(laqta_domain::geometry::PhysRect) -> Option<[f32; 4]> + Send + Sync>;

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
    fn add_custom(
        &self,
        path: PathBuf,
        family: String,
        name: String,
    ) -> laqta_ports::BoxFuture<'static, Result<laqta_domain::settings::CustomModel, String>>;
}

pub type RemoteStores = Arc<dyn Fn() -> Vec<(String, Arc<dyn laqta_ports::RemoteStorage>)> + Send + Sync>;
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
    /// The S3, FTP and SFTP accounts as remote stores, built from the current settings.
    pub remote_storages: RemoteStores,
    /// The model manager (ADR 023).
    pub models: Arc<dyn ModelService>,
    /// Update check and install.
    pub check_update: UpdateCheck,
    pub install_update: UpdateInstall,
    /// The system file and folder pickers (the `Prompt` port).
    pub prompt: Arc<dyn laqta_ports::Prompt>,
    /// Asks the platform to do something to a window that GPUI has no call for. It returns
    /// false where the platform needs nothing done, or can't do it.
    pub native_window: NativeWindowHook,
    /// Maps a physical rect to logical desktop pixels where dividing by the scale is not enough.
    pub logical_rect: Option<LogicalRectHook>,
    /// Run at login.
    pub autostart: Arc<dyn laqta_ports::Autostart>,
    /// "Upload with Laqta" in file managers, Send To, and the browser native-messaging host.
    pub integration: Arc<dyn laqta_ports::ShellIntegration>,
    /// The font handle shared with the Editor engine (ADR 024).
    pub fonts: laqta_editor::Fonts,
    /// The compute pool and blocking pool (`run_compute` for editor operations that need it).
    pub spawner: Arc<dyn laqta_ports::Spawner>,
    pub codecs: laqta_imaging::Codecs,
    /// What the last hotkey registration reported, for the Shortcuts page.
    pub hotkey_report: Arc<dyn Fn() -> HotkeyReport + Send + Sync>,
}

/// The outcome of the last global-hotkey registration.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HotkeyReport {
    /// Shortcuts the system accepted.
    pub registered: Vec<laqta_domain::hotkey::KeyChord>,
    /// Shortcuts the system turned down, with its reason (often: taken by another app).
    pub failed: Vec<(laqta_domain::hotkey::KeyChord, String)>,
    /// Global shortcuts can't work on this system at all, and why.
    pub unsupported: Option<String>,
    /// Registration hasn't finished yet (the portal can take seconds).
    pub pending: bool,
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

gpui::actions!(laqta, [QuitLaqta, OpenSettings, CloseWindow]);

/// Closes `window` for Command+W. On the region overlay that cancels the selection, as Escape
/// does: removing only the window would leave the session waiting. The recording bar, border and
/// countdown stay, because the recording they belong to goes on.
pub(crate) fn close_window(window: gpui::AnyWindowHandle, cx: &mut App) {
    if cx.has_global::<UiState>() {
        let state = cx.global::<UiState>();
        if state.overlay.holds(window) {
            state.overlay.cancel(cx);
            return;
        }
        if state.recording.holds(window) {
            return;
        }
    }
    let _ = window.update(cx, |_, window, _| window.remove_window());
}

/// What a macOS app is expected to have: an application menu with Settings and Quit, their
/// Command shortcuts and Command+W.
fn mac_app_menu(dispatch: Arc<dyn Fn(Action) + Send + Sync>, cx: &mut App) {
    use gpui::{KeyBinding, Menu, MenuItem};
    let (quit, settings) = (dispatch.clone(), dispatch);
    cx.on_action(move |_: &QuitLaqta, _| quit(Action::ExitLaqta));
    cx.on_action(move |_: &OpenSettings, _| settings(Action::OpenSettings));
    cx.on_action(|_: &CloseWindow, cx| {
        if let Some(window) = cx.active_window() {
            close_window(window, cx);
        }
    });
    cx.bind_keys([
        KeyBinding::new("cmd-q", QuitLaqta, None),
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("cmd-w", CloseWindow, None),
    ]);
    cx.set_menus([Menu {
        name: "Laqta".into(),
        items: vec![
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::separator(),
            MenuItem::action("Close Window", CloseWindow),
            MenuItem::action("Quit Laqta", QuitLaqta),
        ],
        disabled: false,
    }]);
}

/// Runs GPUI on the current (main) thread until a `Quit` command arrives. Opens no window by itself.
pub fn run(services: UiServices, rx: UiReceiver) {
    // Laqta lives in the tray: closing the last window must not quit it.
    let app = gpui_platform::application().with_assets(crate::assets::Assets).with_quit_mode(gpui::QuitMode::Explicit);
    if cfg!(target_os = "macos") {
        // A click on the Dock icon while Laqta runs opens the main window.
        let reopen = services.dispatch.clone();
        app.on_reopen(move |_| reopen(Action::OpenMainWindow));
        // "Open With > Laqta" and files dropped on the Dock icon are uploaded.
        let laqta = services.app.clone();
        app.on_open_urls(move |urls| {
            let paths = crate::presenters::open_urls::file_paths(&urls);
            if !paths.is_empty() {
                let _ = laqta.upload_files(paths, None);
            }
        });
    }
    app.run(move |cx: &mut App| {
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
        // Every window but the overlay is a `Root`; log its build and first frame (`perf`).
        cx.observe_new(|_: &mut gpui_component::Root, window: Option<&mut gpui::Window>, _| {
            if let Some(window) = window {
                crate::perf::first_frame("window", window);
                // "System" follows the OS theme while Laqta runs, not only at start.
                window
                    .observe_window_appearance(|window, cx| {
                        let Some(state) = cx.try_global::<UiState>() else { return };
                        let theme = state.services.app.settings().theme;
                        crate::theme::follow_system(theme, window, cx);
                    })
                    .detach();
            }
        })
        .detach();

        if cfg!(target_os = "macos") {
            mac_app_menu(services.dispatch.clone(), cx);
        }

        // Task events feed the task list; this task sleeps until an event arrives.
        let mut events = services.app.events();
        cx.spawn(async move |cx: &mut AsyncApp| {
            loop {
                // A slow reader skips what the channel dropped and carries on.
                let event: TaskEvent = match events.recv().await {
                    Ok(event) => event,
                    Err(async_broadcast::RecvError::Overflowed(skipped)) => {
                        tracing::warn!(skipped, "ui: task events were skipped");
                        // The skipped events may have ended a recording: do not leave its bar up.
                        cx.update(crate::recording_bar::resync);
                        continue;
                    }
                    Err(async_broadcast::RecvError::Closed) => break,
                };
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
    tracing::debug!(target: "laqta::perf", "ui command");
    // The first command that can open a window loads the UI fonts; a tray-only process never does.
    if !matches!(cmd, UiCommand::UpdateOverlay(_) | UiCommand::CloseOverlay | UiCommand::ClosePins | UiCommand::Quit) {
        crate::theme::ensure_fonts(cx);
    }
    match cmd {
        UiCommand::ShowOverlay { frames, images, view, on_input } => overlay::show(frames, images, view, on_input, cx),
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
        UiCommand::Open(ToolWindow::Tool(laqta_ports::ToolKind::ColorPicker)) => crate::color_window::open(None, cx),
        UiCommand::Open(ToolWindow::Tool(kind)) => crate::tools_window::open(kind, cx),
        UiCommand::ShowColor(color) => crate::color_window::open(Some(color), cx),
        UiCommand::Pin(image) => crate::pin_window::pin(image, cx),
        UiCommand::PhotoEditor(image) => crate::photo_window::open(Some(image), None, cx),
        UiCommand::Open(ToolWindow::PhotoEditor(path)) => crate::photo_window::open(None, path, cx),
        UiCommand::ClosePins => crate::pin_window::close_all(cx),
        UiCommand::ShowText { title, text } => {
            crate::text_window::open(crate::text_window::TextResult { title, image: None, text, copied: false }, cx)
        }
        // The OCR route copies non-empty text before it shows the result.
        UiCommand::ShowOcr { image, text } => {
            let copied = !text.trim().is_empty();
            crate::text_window::open(crate::text_window::TextResult { title: "Text capture".into(), image: Some(image), text, copied }, cx)
        }
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
