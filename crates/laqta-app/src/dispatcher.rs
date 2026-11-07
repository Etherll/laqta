use std::fmt;
use std::sync::Arc;

use async_broadcast::{InactiveReceiver, Receiver};
use laqta_domain::{Action, Settings, WorkflowId};
use laqta_ports::{AppError, BoxFuture, Clipboard, Clock, HostInfo, HoverSource, Notifier, Overlay, Rng, ScreenSource, Shell, Spawner, TaskEvent, TaskId};

use crate::files;
use crate::task::{Task, TaskJob};

/// Events kept for a slow listener. When the buffer is full the oldest event is dropped.
const EVENT_CAPACITY: usize = 256;

/// The ports the application uses. `main` builds them and passes them in (ADR 024).
pub struct Ports {
    pub screen: Arc<dyn ScreenSource>,
    pub clipboard: Arc<dyn Clipboard>,
    pub shell: Arc<dyn Shell>,
    pub clock: Arc<dyn Clock>,
    pub spawner: Arc<dyn Spawner>,
    pub overlay: Arc<dyn Overlay>,
    pub hover: Arc<dyn HoverSource>,
    pub notifier: Arc<dyn Notifier>,
    pub rng: Arc<dyn Rng>,
    /// Plain values `main` supplies, not ports.
    pub host: HostInfo,
    pub codecs: laqta_imaging::Codecs,
}

impl fmt::Debug for Ports {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ports").finish_non_exhaustive()
    }
}

/// What a dispatched action produced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dispatched {
    Task(TaskId),
    Done,
    Exit,
}

/// The current settings. A new value replaces the old one; running tasks keep the value they started with.
#[derive(Debug)]
pub(crate) struct SettingsCell(std::sync::RwLock<Arc<Settings>>);

impl SettingsCell {
    pub(crate) fn new(settings: Settings) -> Self {
        SettingsCell(std::sync::RwLock::new(Arc::new(settings)))
    }
    pub(crate) fn get(&self) -> Arc<Settings> {
        match self.0.read() {
            Ok(g) => Arc::clone(&g),
            Err(p) => Arc::clone(&p.into_inner()),
        }
    }
    pub(crate) fn set(&self, settings: Settings) {
        let new = Arc::new(settings);
        match self.0.write() {
            Ok(mut g) => *g = new,
            Err(p) => *p.into_inner() = new,
        }
    }
}

pub struct App {
    ports: Arc<Ports>,
    settings: Arc<SettingsCell>,
    task: Task,
    /// Keeps the event channel open while nobody listens, so sends never fail.
    idle: InactiveReceiver<TaskEvent>,
}

impl fmt::Debug for App {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("App").field("settings", &self.settings).finish_non_exhaustive()
    }
}

impl App {
    pub fn new(ports: Ports, settings: Settings) -> Arc<App> {
        let (mut events, active) = async_broadcast::broadcast::<TaskEvent>(EVENT_CAPACITY);
        events.set_overflow(true);
        let ports = Arc::new(ports);
        let settings = Arc::new(SettingsCell::new(settings));
        let task = Task::new(Arc::clone(&ports), Arc::clone(&settings), events);
        Arc::new(App { ports, settings, task, idle: active.deactivate() })
    }

    pub fn dispatch(self: &Arc<Self>, action: Action, _workflow: Option<WorkflowId>) -> BoxFuture<'static, Result<Dispatched, AppError>> {
        Box::pin(Arc::clone(self).route(action))
    }

    /// A new listener. It sees the events sent after this call.
    pub fn events(&self) -> Receiver<TaskEvent> {
        self.idle.activate_cloned()
    }

    pub fn settings(&self) -> Settings {
        self.settings.get().as_ref().clone()
    }

    /// Replaces the settings. New dispatches and new tasks use them.
    pub fn set_settings(&self, settings: Settings) {
        self.settings.set(settings);
    }

    async fn route(self: Arc<Self>, action: Action) -> Result<Dispatched, AppError> {
        match action {
            Action::PrintScreen => {
                let frames = self.ports.screen.capture_all().await.map_err(AppError::from)?;
                Ok(Dispatched::Task(self.task.submit(TaskJob::Capture { frames })))
            }
            Action::OpenScreenshotsFolder => {
                let dir = self.settings.get().screenshots_dir.clone();
                files::create_dir_all(self.ports.spawner.as_ref(), dir.clone())
                    .await
                    .map_err(|e| AppError::Io(e.to_string()))?;
                self.ports.shell.open_path(&dir).map_err(AppError::from)?;
                Ok(Dispatched::Done)
            }
            Action::ExitLaqta => Ok(Dispatched::Exit),
            other => Err(AppError::Unsupported(format!("{other:?}"))),
        }
    }
}
