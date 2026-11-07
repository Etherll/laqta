use std::fmt;
use std::sync::Arc;

use async_broadcast::{InactiveReceiver, Receiver};
use laqta_domain::Frame;
use laqta_domain::geometry::PhysRect;
use laqta_domain::region::HoverTarget;
use laqta_domain::workflow::TaskSettings;
use laqta_domain::{Action, Settings, WorkflowId};
use laqta_ports::{
    AppError, BoxFuture, CaptureRequest, Clipboard, Clock, HostInfo, HoverSource, Notifier, Overlay, PortError, Rng, ScreenSource, Shell,
    Spawner, TaskEvent, TaskId, ToolHost, ToolWindow,
};

use crate::files;
use crate::region;
use crate::task::{CaptureImage, CaptureInfo};
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
    pub tools: Arc<dyn ToolHost>,
    pub prompt: Arc<dyn laqta_ports::Prompt>,
    pub uploaders: Arc<dyn laqta_ports::Uploaders>,
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
    /// `DisableHotkeys` flipped `settings.disable_hotkeys`; `main` re-registers hotkeys.
    HotkeysToggled(bool),
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
    /// The last confirmed region selection, for `LastRegion`.
    last_region: std::sync::Mutex<Option<(Vec<PhysRect>, Option<HoverTarget>)>>,
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
        Arc::new(App { ports, settings, task, last_region: std::sync::Mutex::new(None), idle: active.deactivate() })
    }

    pub fn dispatch(self: &Arc<Self>, action: Action, workflow: Option<WorkflowId>) -> BoxFuture<'static, Result<Dispatched, AppError>> {
        Box::pin(Arc::clone(self).route(action, workflow))
    }

    /// Stops one task (queued tasks never run; running ones stop at the next step).
    pub fn stop(&self, id: TaskId) -> bool {
        self.task.stop(id)
    }

    pub fn stop_all(&self) {
        self.task.stop_all();
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

    async fn frames(&self, ts: &TaskSettings) -> Result<Vec<Frame>, AppError> {
        if ts.capture.delay_ms > 0 {
            self.ports.clock.sleep(std::time::Duration::from_millis(ts.capture.delay_ms as u64)).await;
        }
        self.ports.screen.capture(CaptureRequest { show_cursor: ts.capture.show_cursor }).await.map_err(AppError::from)
    }

    fn submit(&self, image: CaptureImage, settings: TaskSettings, target: Option<&HoverTarget>) -> Dispatched {
        let info = target.map(|t| CaptureInfo { window_title: t.title.clone(), process_name: t.process.clone() }).unwrap_or_default();
        Dispatched::Task(self.task.submit(TaskJob::Capture { image, settings, info }))
    }

    async fn crop(
        &self,
        frames: Vec<Frame>,
        rects: Vec<PhysRect>,
        target: Option<HoverTarget>,
        ts: TaskSettings,
    ) -> Result<Dispatched, AppError> {
        let sel = region::crop_frames(&self.ports, frames, rects, target).await?;
        Ok(self.submit(CaptureImage::Image(sel.image), ts, sel.target.as_ref()))
    }

    /// A window capture. With `transparent_window` on, the window's own bitmap (its rounded corners and
    /// shadow included) when the platform can take it. Otherwise the crop of `frames` to the window's rect.
    async fn window(&self, frames: Vec<Frame>, target: HoverTarget, ts: TaskSettings) -> Result<Dispatched, AppError> {
        if ts.capture.transparent_window {
            let request = CaptureRequest { show_cursor: ts.capture.show_cursor };
            match self.ports.screen.capture_window(&target, request).await {
                Ok(bitmap) => return Ok(self.submit(CaptureImage::Image(bitmap), ts, Some(&target))),
                Err(PortError::Unsupported(_)) => {}
                Err(error) => return Err(AppError::from(error)),
            }
        }
        self.crop(frames, vec![target.rect], Some(target), ts).await
    }

    async fn region(&self, ts: TaskSettings) -> Result<Dispatched, AppError> {
        let frames = self.frames(&ts).await?;
        let sel = region::select(&self.ports, frames, &ts.capture.region).await?;
        if let Ok(mut last) = self.last_region.lock() {
            *last = Some((sel.rects.clone(), sel.target.clone()));
        }
        Ok(self.submit(CaptureImage::Image(sel.image), ts, sel.target.as_ref()))
    }

    fn open(&self, window: ToolWindow) -> Result<Dispatched, AppError> {
        self.ports.tools.open(window).map_err(AppError::from)?;
        Ok(Dispatched::Done)
    }

    async fn route(self: Arc<Self>, action: Action, workflow: Option<WorkflowId>) -> Result<Dispatched, AppError> {
        let settings = self.settings.get();
        let ts = settings.task_settings(workflow);
        match action {
            Action::PrintScreen => {
                let frames = self.frames(&ts).await?;
                Ok(self.submit(CaptureImage::Frames(frames), ts, None))
            }
            Action::ActiveMonitor => {
                let frames = self.frames(&ts).await?;
                // The monitor under the focused window's centre; else the primary monitor.
                let anchor = self
                    .ports
                    .hover
                    .active_window()
                    .await
                    .ok()
                    .flatten()
                    .map(|w| (w.rect.x + (w.rect.width / 2) as i32, w.rect.y + (w.rect.height / 2) as i32));
                let contains = |f: &&Frame, (x, y): (i32, i32)| {
                    let r = f.monitor.rect;
                    x >= r.x && x < r.right() && y >= r.y && y < r.bottom()
                };
                let pick = anchor
                    .and_then(|p| frames.iter().find(|f| contains(f, p)))
                    .or_else(|| frames.iter().find(|f| f.monitor.primary))
                    .or_else(|| frames.first())
                    .cloned()
                    .ok_or_else(|| AppError::Io("no monitors".into()))?;
                Ok(self.submit(CaptureImage::Frames(vec![pick]), ts, None))
            }
            Action::ActiveWindow => {
                let frames = self.frames(&ts).await?;
                let target = self
                    .ports
                    .hover
                    .active_window()
                    .await
                    .map_err(AppError::from)?
                    .ok_or_else(|| AppError::Unsupported("no active window".into()))?;
                self.window(frames, target, ts).await
            }
            Action::CustomWindow => {
                let title = ts.capture.custom_window_title.to_lowercase();
                if title.is_empty() {
                    return Err(AppError::Unsupported("set a window title for \"Capture window by title\" in settings".into()));
                }
                let frames = self.frames(&ts).await?;
                let targets = self.ports.hover.targets().await.map_err(AppError::from)?;
                let target = targets
                    .into_iter()
                    .find(|t| t.title.to_lowercase().contains(&title))
                    .ok_or_else(|| AppError::Io(format!("no window titled like \"{}\"", ts.capture.custom_window_title)))?;
                self.window(frames, target, ts).await
            }
            Action::RectangleRegion | Action::RectangleLight | Action::RectangleTransparent => self.region(ts).await,
            Action::CustomRegion => {
                let rect = ts.capture.custom_region.ok_or_else(|| AppError::Unsupported("set a custom region in settings".into()))?;
                let frames = self.frames(&ts).await?;
                self.crop(frames, vec![rect], None, ts).await
            }
            Action::LastRegion => {
                let last = self.last_region.lock().ok().and_then(|l| l.clone());
                match last {
                    Some((rects, target)) => {
                        let frames = self.frames(&ts).await?;
                        self.crop(frames, rects, target, ts).await
                    }
                    None => self.region(ts).await,
                }
            }
            Action::OpenScreenshotsFolder => {
                let dir = settings.screenshots_dir_for(&ts);
                files::create_dir_all(self.ports.spawner.as_ref(), dir.clone()).await.map_err(|e| AppError::Io(e.to_string()))?;
                self.ports.shell.open_path(&dir).map_err(AppError::from)?;
                Ok(Dispatched::Done)
            }
            Action::OpenMainWindow => self.open(ToolWindow::Main),
            Action::OpenSettings => self.open(ToolWindow::Settings),
            Action::OpenHistory => self.open(ToolWindow::History),
            Action::OpenImageHistory => self.open(ToolWindow::ImageHistory),
            Action::ToggleTrayMenu => self.open(ToolWindow::TrayMenu),
            Action::ToggleActionsToolbar => self.open(ToolWindow::ActionsToolbar),
            Action::DisableHotkeys => {
                let mut next = settings.as_ref().clone();
                next.disable_hotkeys = !next.disable_hotkeys;
                let off = next.disable_hotkeys;
                self.settings.set(next);
                Ok(Dispatched::HotkeysToggled(off))
            }
            Action::StopUploads => {
                self.task.stop_all();
                Ok(Dispatched::Done)
            }
            Action::ExitLaqta => Ok(Dispatched::Exit),
            other => Err(AppError::Unsupported(format!("{} is not available yet", other.label()))),
        }
    }
}
