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
use crate::{history_window, main_window, overlay, settings_window, toast};

pub(crate) enum UiCommand {
    ShowOverlay { frames: Vec<Frame>, view: RegionView, on_input: InputCallback },
    UpdateOverlay(RegionView),
    CloseOverlay,
    Toast(Toast),
    Open(ToolWindow),
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

impl ToolHost for UiHandle {
    fn open(&self, window: ToolWindow) -> Result<(), PortError> {
        self.send(UiCommand::Open(window))
    }
}

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
}

impl gpui::Global for UiState {}

/// Runs GPUI on the current (main) thread until a `Quit` command arrives. Opens no window by itself.
pub fn run(services: UiServices, rx: UiReceiver) {
    gpui_platform::application().run(move |cx: &mut App| {
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
        });

        // Task events feed the task list; this task sleeps until an event arrives.
        let mut events = services.app.events();
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Ok(event) = events.recv().await {
                let event: TaskEvent = event;
                cx.update(|cx| tasks.update(cx, |list, cx| {
                    list.apply(&event);
                    cx.notify();
                }));
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
