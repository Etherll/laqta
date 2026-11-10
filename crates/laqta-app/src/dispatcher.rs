use std::fmt;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::Arc;

use async_broadcast::{InactiveReceiver, Receiver};
use futures_channel::oneshot;
use laqta_domain::geometry::PhysRect;
use laqta_domain::region::{HoverTarget, SelectionAction};
use laqta_domain::workflow::{AfterCaptureTasks, TaskSettings};
use laqta_domain::{Action, Settings, WorkflowId};
use laqta_domain::{Bitmap, Frame};
use laqta_ports::{
    AppError, BoxFuture, CaptureRequest, Clipboard, ClipboardContent, Clock, HostInfo, HoverSource, Notifier, Overlay, PortError, Rng,
    ScreenSource, Shell, Spawner, TaskEvent, TaskId, ToolHost, ToolWindow,
};

use crate::files;
use crate::recording::{ActiveRecording, Area};
use crate::region;
use crate::task::compute;
use crate::task::{CaptureImage, CaptureInfo};
use crate::task::{Task, TaskJob, UploadSource};

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
    pub editor: Arc<dyn laqta_ports::EditorHost>,
    pub media: Arc<dyn laqta_ports::Media>,
    pub stream: Arc<dyn laqta_ports::ScreenStream>,
    pub ocr: Arc<dyn laqta_ports::Ocr>,
    pub window_ops: Arc<dyn laqta_ports::WindowOps>,
    /// Synthetic scroll input for scrolling capture.
    pub input: Arc<dyn laqta_ports::InputSynth>,
    /// The mouse highlighter.
    pub highlighter: Arc<dyn laqta_ports::PointerHighlighter>,
    /// Background removal, image analysis and inpainting.
    pub background: Arc<dyn laqta_ports::BackgroundRemover>,
    pub analyzer: Arc<dyn laqta_ports::ImageAnalyzer>,
    pub inpainter: Arc<dyn laqta_ports::Inpainter>,
    /// Watch folders.
    pub watcher: Arc<dyn laqta_ports::FileWatcher>,
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

/// What a delivery mechanism already holds when it dispatches an action (ADR 015). The upload actions ask the user
/// for a file, text or a URL; with the matching input they use it and skip the question. A watch folder, a drop, the
/// CLI and IPC dispatch this way, so every one of them goes through the same routes. Any other input is ignored.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ActionInput {
    #[default]
    None,
    /// For `FileUpload`: one upload job per file.
    Files(Vec<PathBuf>),
    /// For `UploadText`: the text to upload.
    Text(String),
    /// For `UploadUrl` and `ShortenUrl`: the URL to re-upload or to shorten.
    Url(String),
}

/// What a dispatched action produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dispatched {
    Task(TaskId),
    /// Several files: one task each, in order.
    Tasks(Vec<TaskId>),
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
    pub(crate) ports: Arc<Ports>,
    pub(crate) settings: Arc<SettingsCell>,
    pub(crate) task: Task,
    /// The last confirmed region selection, for `LastRegion`.
    last_region: std::sync::Mutex<Option<(Vec<PhysRect>, Option<HoverTarget>)>>,
    /// The recording that is starting or running (one at a time). Its driver clears it when the recording ends.
    pub(crate) recording: std::sync::Mutex<Option<ActiveRecording>>,
    /// The area of the last recording, for `StartScreenRecorder`.
    pub(crate) last_recording_area: std::sync::Mutex<Option<PhysRect>>,
    /// The running auto capture's stop flag.
    pub(crate) auto_capture: std::sync::Mutex<Option<Arc<std::sync::atomic::AtomicBool>>>,
    /// Live watch-folder handles; replaced by `start_watch_folders`.
    pub(crate) watches: std::sync::Mutex<Vec<laqta_ports::WatchHandle>>,
    /// What the borderless and top-most toggles switched on.
    pub(crate) window_toggles: crate::tools::WindowToggles,
    /// The history the windows edit, once `main` attaches it (`history.rs`).
    pub(crate) history: std::sync::OnceLock<Arc<dyn laqta_ports::HistoryEdit>>,
    /// Runs effect presets with text and assets (`effects.rs`). Task holds the same one.
    pub(crate) effects: Arc<crate::effects::EffectRunner>,
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
        let effects = Arc::new(crate::effects::EffectRunner::default());
        let task = Task::new(Arc::clone(&ports), Arc::clone(&settings), Arc::clone(&effects), events);
        Arc::new(App {
            ports,
            settings,
            task,
            last_region: std::sync::Mutex::new(None),
            recording: std::sync::Mutex::new(None),
            last_recording_area: std::sync::Mutex::new(None),
            auto_capture: std::sync::Mutex::new(None),
            watches: std::sync::Mutex::new(Vec::new()),
            window_toggles: Default::default(),
            history: std::sync::OnceLock::new(),
            effects,
            idle: active.deactivate(),
        })
    }

    pub fn dispatch(self: &Arc<Self>, action: Action, workflow: Option<WorkflowId>) -> BoxFuture<'static, Result<Dispatched, AppError>> {
        self.dispatch_with(action, ActionInput::None, workflow)
    }

    /// `dispatch` for a delivery mechanism that already has the action's input (see `ActionInput`).
    pub fn dispatch_with(
        self: &Arc<Self>,
        action: Action,
        input: ActionInput,
        workflow: Option<WorkflowId>,
    ) -> BoxFuture<'static, Result<Dispatched, AppError>> {
        tracing::debug!(target: "laqta::perf", ?action, "dispatch");
        Box::pin(Arc::clone(self).route(action, input, workflow))
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

    pub(crate) async fn frames(&self, ts: &TaskSettings) -> Result<Vec<Frame>, AppError> {
        if ts.capture.delay_ms > 0 {
            self.ports.clock.sleep(std::time::Duration::from_millis(ts.capture.delay_ms as u64)).await;
        }
        self.ports.screen.capture(CaptureRequest { show_cursor: ts.capture.show_cursor }).await.map_err(AppError::from)
    }

    pub(crate) fn submit(&self, image: CaptureImage, settings: TaskSettings, target: Option<&HoverTarget>) -> Dispatched {
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

    async fn region(&self, mut ts: TaskSettings) -> Result<Dispatched, AppError> {
        let frames = self.frames(&ts).await?;
        let sel = region::select(&self.ports, frames, &ts.capture.region).await?;
        ts.after_capture = after_capture_for(ts.after_capture, sel.action);
        if sel.annotate {
            ts.after_capture.insert(AfterCaptureTasks::ANNOTATE_IMAGE);
        }
        if let Ok(mut last) = self.last_region.lock() {
            *last = Some((sel.rects.clone(), sel.target.clone()));
        }
        Ok(self.submit(CaptureImage::Image(sel.image), ts, sel.target.as_ref()))
    }

    fn open(&self, window: ToolWindow) -> Result<Dispatched, AppError> {
        self.ports.tools.open(window).map_err(AppError::from)?;
        Ok(Dispatched::Done)
    }

    /// Submits an Upload job.
    fn upload(&self, source: UploadSource, settings: TaskSettings) -> Dispatched {
        Dispatched::Task(self.task.submit(TaskJob::Upload { source, settings }))
    }

    /// Submits one File upload job per path, in order, with the workflow's settings, and returns their ids. The
    /// file and folder routes, drag-and-drop and the CLI all use it.
    pub fn upload_files(self: &Arc<Self>, paths: Vec<PathBuf>, workflow: Option<WorkflowId>) -> Vec<TaskId> {
        let ts = self.settings.get().task_settings(workflow);
        paths.into_iter().map(|path| self.task.submit(TaskJob::Upload { source: UploadSource::File(path), settings: ts.clone() })).collect()
    }

    /// Submits one upload per file for `paths`, with each folder expanded to the files inside it, recursively. The
    /// file routes, the folder route, the clipboard, the CLI and Send To all come here. A declined or cancelled
    /// confirmation submits nothing.
    pub async fn upload_paths(self: &Arc<Self>, paths: Vec<PathBuf>, workflow: Option<WorkflowId>) -> Result<Vec<TaskId>, AppError> {
        let files = files::files_under(self.ports.spawner.as_ref(), paths).await.map_err(|error| AppError::Io(error.to_string()))?;
        self.confirm_upload_many(files, workflow).await
    }

    /// Submits `files` as one upload each. Above `MULTI_UPLOAD_CONFIRM_ABOVE` files the user confirms first, as ShareX
    /// does before a multi-file upload. A declined or cancelled confirmation submits nothing.
    async fn confirm_upload_many(self: &Arc<Self>, files: Vec<PathBuf>, workflow: Option<WorkflowId>) -> Result<Vec<TaskId>, AppError> {
        if files.len() > MULTI_UPLOAD_CONFIRM_ABOVE {
            let text = format!("Upload {} files?", files.len());
            let confirmed = match self.ports.prompt.confirm("Upload files", &text).await {
                Ok(confirmed) => confirmed,
                Err(PortError::Cancelled) => false,
                // No way to ask: upload, as ShareX does with its warning turned off.
                Err(PortError::Unsupported(_)) => true,
                Err(error) => return Err(AppError::from(error)),
            };
            if !confirmed {
                return Ok(Vec::new());
            }
        }
        Ok(self.upload_files(files, workflow))
    }

    /// Submits one Upload job with the workflow's settings: text, a file, a URL to re-upload or a URL to
    /// shorten. The CLI, IPC and native-messaging requests use it.
    pub fn upload_source(self: &Arc<Self>, source: UploadSource, workflow: Option<WorkflowId>) -> TaskId {
        let ts = self.settings.get().task_settings(workflow);
        self.task.submit(TaskJob::Upload { source, settings: ts })
    }

    /// The files the user picks. Without a file dialog the user types a path instead. `None` when the user
    /// cancels.
    async fn picked_files(&self, title: &str) -> Result<Option<Vec<PathBuf>>, AppError> {
        match self.ports.prompt.pick_files(title).await {
            Ok(paths) => Ok(Some(paths)),
            Err(PortError::Unsupported(_)) => {
                let answer = self.ports.prompt.ask_text(title, "").await.map_err(AppError::from)?;
                let path = answer.trim();
                if path.is_empty() {
                    return Err(AppError::Unsupported("no file given".into()));
                }
                Ok(Some(vec![PathBuf::from(path)]))
            }
            Err(PortError::Cancelled) => Ok(None),
            Err(error) => Err(AppError::from(error)),
        }
    }

    /// The folder the user picks. `None` when the user cancels.
    async fn picked_folder(&self) -> Result<Option<PathBuf>, AppError> {
        match self.ports.prompt.pick_folder(Action::FolderUpload.label()).await {
            Ok(folder) => Ok(Some(folder)),
            Err(PortError::Cancelled) => Ok(None),
            Err(error) => Err(AppError::from(error)),
        }
    }

    /// The image beautifier: one picked image is decoded and beautified on the compute pool, then submitted as a
    /// capture that is saved and copied. The beautifier's flags are the job's; the workflow's capture flags are not used.
    async fn beautify_file(&self, mut ts: TaskSettings) -> Result<Dispatched, AppError> {
        let Some(paths) = self.picked_files(Action::ImageBeautifier.label()).await? else { return Ok(Dispatched::Done) };
        let Some(path) = paths.into_iter().next() else { return Ok(Dispatched::Done) };
        let bytes = files::read(self.ports.spawner.as_ref(), path).await.map_err(|error| AppError::Io(error.to_string()))?;
        let codecs = self.ports.codecs.clone();
        let beautify = ts.beautify.clone();
        let work = move || -> Result<Bitmap, AppError> {
            let decoded = codecs.decode(&bytes).map_err(|error| AppError::Io(error.to_string()))?;
            Ok(laqta_imaging::beautify::beautify(&decoded, &beautify))
        };
        let image = compute(self.ports.spawner.as_ref(), work).await?;
        ts.after_capture = AfterCaptureTasks::SAVE_IMAGE_TO_FILE | AfterCaptureTasks::COPY_IMAGE_TO_CLIPBOARD;
        Ok(self.submit(CaptureImage::Image(image), ts, None))
    }

    /// Reads the clipboard on the blocking pool, so the OS clipboard never stalls the async worker. A panic in the
    /// read resumes here, as it would have from the read itself.
    async fn clipboard_get(&self) -> Result<Option<ClipboardContent>, PortError> {
        let clipboard = Arc::clone(&self.ports.clipboard);
        let (tx, rx) = oneshot::channel();
        self.ports
            .spawner
            .run_blocking(Box::new(move || {
                let outcome = std::panic::catch_unwind(AssertUnwindSafe(move || clipboard.get()));
                let _ = tx.send(outcome);
            }))
            .await;
        match rx.await {
            Ok(Ok(result)) => result,
            Ok(Err(payload)) => std::panic::resume_unwind(payload),
            Err(oneshot::Canceled) => Err(PortError::Cancelled),
        }
    }

    /// "Upload from clipboard": an image becomes a capture with the upload flag, each file is one upload, a URL
    /// is downloaded and uploaded, and other text is uploaded as text. An empty clipboard is an error.
    async fn clipboard_upload(self: &Arc<Self>, mut ts: TaskSettings, workflow: Option<WorkflowId>) -> Result<Dispatched, AppError> {
        match self.clipboard_get().await.map_err(AppError::from)? {
            Some(ClipboardContent::Image(image)) => {
                ts.after_capture.insert(AfterCaptureTasks::UPLOAD_IMAGE_TO_HOST);
                Ok(self.submit(CaptureImage::Image(image), ts, None))
            }
            Some(ClipboardContent::Files(paths)) if !paths.is_empty() => Ok(first_task(self.upload_paths(paths, workflow).await?)),
            Some(ClipboardContent::Text(text)) if !text.is_empty() => {
                let url = as_url(&text).map(str::to_owned);
                let source = match url {
                    Some(url) => UploadSource::Url(url),
                    None => UploadSource::Text(text),
                };
                Ok(self.upload(source, ts))
            }
            _ => Err(AppError::Unsupported("clipboard is empty".into())),
        }
    }

    async fn route(self: Arc<Self>, action: Action, input: ActionInput, workflow: Option<WorkflowId>) -> Result<Dispatched, AppError> {
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
            Action::UploadText => {
                let text = match input {
                    ActionInput::Text(text) => text,
                    _ => match self.ports.clipboard.get().ok().flatten() {
                        Some(ClipboardContent::Text(text)) => text,
                        _ => self.ports.prompt.ask_text("Upload text", "").await.map_err(AppError::from)?,
                    },
                };
                if text.is_empty() {
                    return Err(AppError::Unsupported("nothing to upload".into()));
                }
                Ok(self.upload(UploadSource::Text(text), ts))
            }
            Action::ClipboardUpload | Action::ClipboardUploadWithContentViewer => self.clipboard_upload(ts, workflow).await,
            Action::UploadUrl => {
                let answer = match input {
                    ActionInput::Url(url) => url,
                    _ => self.ports.prompt.ask_text("URL to upload", "").await.map_err(AppError::from)?,
                };
                let url = as_url(&answer).ok_or_else(not_a_url)?.to_owned();
                Ok(self.upload(UploadSource::Url(url), ts))
            }
            Action::ShortenUrl => {
                let url = match input {
                    ActionInput::Url(url) => as_url(&url).ok_or_else(not_a_url)?.to_owned(),
                    _ => {
                        let clipboard = match self.ports.clipboard.get().ok().flatten() {
                            Some(ClipboardContent::Text(text)) => as_url(&text).map(str::to_owned),
                            _ => None,
                        };
                        match clipboard {
                            Some(url) => url,
                            None => {
                                let answer = self.ports.prompt.ask_text(Action::ShortenUrl.label(), "").await.map_err(AppError::from)?;
                                as_url(&answer).ok_or_else(not_a_url)?.to_owned()
                            }
                        }
                    }
                };
                Ok(self.upload(UploadSource::ShortenUrl(url), ts))
            }
            Action::FileUpload => {
                let paths = match input {
                    ActionInput::Files(paths) => paths,
                    _ => match self.picked_files(Action::FileUpload.label()).await? {
                        Some(paths) => paths,
                        None => return Ok(Dispatched::Done),
                    },
                };
                Ok(first_task(self.upload_paths(paths, workflow).await?))
            }
            Action::FolderUpload => {
                let Some(folder) = self.picked_folder().await? else { return Ok(Dispatched::Done) };
                let paths =
                    files::files_under(self.ports.spawner.as_ref(), vec![folder]).await.map_err(|error| AppError::Io(error.to_string()))?;
                if paths.is_empty() {
                    return Err(AppError::Unsupported("the folder has no files".into()));
                }
                Ok(first_task(self.confirm_upload_many(paths, workflow).await?))
            }
            Action::DragDropUpload => self.open(ToolWindow::Main),
            Action::ImageEditor => self.open(ToolWindow::Editor(None)),
            Action::ImageEffects => self.open(ToolWindow::Effects(None)),
            Action::ImageBeautifier => self.beautify_file(ts).await,
            Action::StopUploads => {
                self.task.stop_all();
                Ok(Dispatched::Done)
            }
            Action::ScreenRecorder => self.record(Area::Region, false, ts).await,
            Action::ScreenRecorderActiveWindow => self.record(Area::ActiveWindow, false, ts).await,
            Action::ScreenRecorderCustomRegion => self.record(Area::CustomRegion, false, ts).await,
            Action::StartScreenRecorder => self.record(Area::LastRecording, false, ts).await,
            Action::ScreenRecorderGif => self.record(Area::Region, true, ts).await,
            Action::ScreenRecorderGifActiveWindow => self.record(Area::ActiveWindow, true, ts).await,
            Action::ScreenRecorderGifCustomRegion => self.record(Area::CustomRegion, true, ts).await,
            Action::StartScreenRecorderGif => self.record(Area::LastRecording, true, ts).await,
            Action::StopScreenRecording => {
                self.recording_stop();
                Ok(Dispatched::Done)
            }
            Action::PauseScreenRecording => {
                self.recording_toggle_pause();
                Ok(Dispatched::Done)
            }
            Action::AbortScreenRecording => {
                self.recording_abort();
                Ok(Dispatched::Done)
            }
            Action::ExitLaqta => Ok(Dispatched::Exit),
            a if let Some(r) = self.guarded_tool_route(a, &ts).await => r,
            Action::ScrollingCapture => self.scrolling_capture(ts, workflow).await,
            Action::AutoCapture => self.auto_capture(None),
            Action::StartAutoCapture => self.auto_capture(Some(true)),
            Action::StopAutoCapture => self.auto_capture(Some(false)),
            other => Err(AppError::Unsupported(format!("{} is not available yet", other.label()))),
        }
    }
}

/// The number of files above which an upload asks the user first.
const MULTI_UPLOAD_CONFIRM_ABOVE: usize = 10;

/// The dispatched result for a batch of jobs: the first job's id, or nothing to wait for when the batch is empty.
fn first_task(ids: Vec<TaskId>) -> Dispatched {
    match ids.as_slice() {
        [] => Dispatched::Done,
        [id] => Dispatched::Task(*id),
        _ => Dispatched::Tasks(ids),
    }
}

/// The after-capture steps for a region the user finished with `action` (ADR 011: what happens to a
/// capture is Task flags). A button on the capture toolbar replaces the configured steps with its own;
/// the after-upload steps are not touched. `Edit` adds the editor to the configured steps, and
/// `Default` leaves them as they are.
fn after_capture_for(configured: AfterCaptureTasks, action: SelectionAction) -> AfterCaptureTasks {
    match action {
        SelectionAction::Default => configured,
        SelectionAction::Copy => AfterCaptureTasks::COPY_IMAGE_TO_CLIPBOARD,
        SelectionAction::Save => AfterCaptureTasks::SAVE_IMAGE_TO_FILE,
        SelectionAction::Pin => AfterCaptureTasks::PIN_TO_SCREEN,
        SelectionAction::Upload => AfterCaptureTasks::SAVE_IMAGE_TO_FILE | AfterCaptureTasks::UPLOAD_IMAGE_TO_HOST,
        SelectionAction::Edit => configured | AfterCaptureTasks::ANNOTATE_IMAGE,
        SelectionAction::CopyText => AfterCaptureTasks::DO_OCR,
    }
}

/// The trimmed text when it is an http or https URL with no spaces in it.
fn as_url(text: &str) -> Option<&str> {
    let url = text.trim();
    let lower = url.to_ascii_lowercase();
    let is_web = lower.starts_with("http://") || lower.starts_with("https://");
    (is_web && !url.contains(char::is_whitespace)).then_some(url)
}

fn not_a_url() -> AppError {
    AppError::Unsupported("not a URL".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A configuration that differs from every action's flags, so a result that kept it shows.
    fn configured() -> AfterCaptureTasks {
        AfterCaptureTasks::SAVE_IMAGE_TO_FILE | AfterCaptureTasks::COPY_IMAGE_TO_CLIPBOARD | AfterCaptureTasks::BEAUTIFY_IMAGE
    }

    #[test]
    fn default_leaves_the_configured_steps_alone() {
        assert_eq!(after_capture_for(configured(), SelectionAction::Default), configured());
        assert_eq!(after_capture_for(AfterCaptureTasks::empty(), SelectionAction::Default), AfterCaptureTasks::empty());
    }

    #[test]
    fn each_toolbar_action_replaces_the_configured_steps() {
        for (action, flags) in [
            (SelectionAction::Copy, AfterCaptureTasks::COPY_IMAGE_TO_CLIPBOARD),
            (SelectionAction::Save, AfterCaptureTasks::SAVE_IMAGE_TO_FILE),
            (SelectionAction::Pin, AfterCaptureTasks::PIN_TO_SCREEN),
            (SelectionAction::Upload, AfterCaptureTasks::SAVE_IMAGE_TO_FILE | AfterCaptureTasks::UPLOAD_IMAGE_TO_HOST),
            (SelectionAction::CopyText, AfterCaptureTasks::DO_OCR),
        ] {
            assert_eq!(after_capture_for(configured(), action), flags, "{action:?}");
            assert_eq!(after_capture_for(AfterCaptureTasks::empty(), action), flags, "{action:?} from nothing");
        }
    }

    #[test]
    fn edit_adds_the_editor_to_the_configured_steps() {
        assert_eq!(after_capture_for(configured(), SelectionAction::Edit), configured() | AfterCaptureTasks::ANNOTATE_IMAGE);
        assert_eq!(after_capture_for(AfterCaptureTasks::empty(), SelectionAction::Edit), AfterCaptureTasks::ANNOTATE_IMAGE);
    }
}
