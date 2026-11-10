//! The Recording session (ARCHITECTURE §2.2 and §4.4). A start action picks the area (a drawn region, the active
//! window, the custom region or the last recording's area), counts down, opens the screen stream and starts the
//! encoder. A driver future (`drive.rs`) then owns the encoder: it takes pause, resume, stop and abort commands,
//! ticks every second, and watches the encoder. The finished file goes to Task.
//!
//! One recording runs at a time. Its slot is claimed before the area is chosen, so a second start stops the
//! recording instead of starting another one.
//!
//! After the claim, every start ends in a `RecordingFailed` or a `RecordingStarted`, and the slot is always freed. A
//! stop or abort that arrives while the area is chosen or the stream opens (a portal dialog nobody answers) cancels
//! that step instead of waiting for it.

mod drive;

use std::any::Any;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::{Arc, MutexGuard, PoisonError};
use std::time::Duration;

use futures_channel::mpsc;
use futures_lite::{FutureExt, StreamExt, future};
use laqta_domain::geometry::PhysRect;
use laqta_domain::record::{AudioDevice, RecordSpec};
use laqta_domain::workflow::{TaskSettings, ToastOptions};
use laqta_ports::{AppError, RecordingHandle, TaskEvent, TaskId, Toast};

use crate::dispatcher::{App, Dispatched};
use crate::files;
use crate::region;

/// One second: the spacing of the countdown and of `RecordingTick`.
const SECOND: Duration = Duration::from_secs(1);
/// The device name the system-sound (loopback) input carries.
const LOOPBACK: &str = "loopback";

/// What a start action records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Area {
    /// An area the user draws on the screen.
    Region,
    /// The active window's rectangle.
    ActiveWindow,
    /// `capture.custom_region` from the settings.
    CustomRegion,
    /// The last recording's area, or a drawn one when there is none.
    LastRecording,
}

/// What the control methods send to the driver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Control {
    Pause,
    Resume,
    /// Pauses a running recording, or resumes a paused one.
    TogglePause,
    Stop,
    Abort,
}

/// The recording that is starting or running: its task id and the channel its driver reads.
pub(crate) struct ActiveRecording {
    id: TaskId,
    control: mpsc::UnboundedSender<Control>,
}

/// A started recording, handed to its driver.
struct Live {
    handle: Box<dyn RecordingHandle>,
    output: PathBuf,
    commands: mpsc::UnboundedReceiver<Control>,
    /// The settings the file goes to Task with. They are the ones the start action ran with.
    ts: TaskSettings,
    /// Stop after this many seconds of recording (0 = until stopped).
    fixed_secs: u32,
}

impl App {
    /// Whether a recording is running, or starting (its area is being chosen, or it is counting down).
    pub fn is_recording(&self) -> bool {
        self.slot().is_some()
    }

    /// Pauses the current recording, if any.
    pub fn recording_pause(&self) {
        self.send_control(Control::Pause);
    }

    /// Resumes the current recording, if it is paused.
    pub fn recording_resume(&self) {
        self.send_control(Control::Resume);
    }

    /// Pauses a running recording, or resumes a paused one (the pause action).
    pub fn recording_toggle_pause(&self) {
        self.send_control(Control::TogglePause);
    }

    /// Stops the current recording; the file goes to Task.
    pub fn recording_stop(&self) {
        self.send_control(Control::Stop);
    }

    /// Stops and discards the current recording.
    pub fn recording_abort(&self) {
        self.send_control(Control::Abort);
    }

    /// A start action. Starting while a recording runs (or starts) stops it instead.
    pub(crate) async fn record(self: Arc<Self>, area: Area, gif: bool, ts: TaskSettings) -> Result<Dispatched, AppError> {
        let (control, commands) = mpsc::unbounded();
        let Some(id) = self.claim(control) else {
            return Ok(Dispatched::Done);
        };
        let toast = ts.general.toast.clone();
        match self.begin(id, commands, area, gif, ts).await {
            Ok(Some(live)) => {
                let app = Arc::clone(&self);
                let toast = live.ts.general.toast.clone();
                self.ports.spawner.spawn(Box::pin(async move {
                    // A panic in the driver ends the recording as a failure and frees the slot (ADR 020).
                    if let Err(payload) = AssertUnwindSafe(drive::run(Arc::clone(&app), id, live)).catch_unwind().await {
                        app.release(id);
                        app.emit_failed(id, AppError::Panic(panic_message(&*payload)), None, &toast);
                    }
                }));
                Ok(Dispatched::Done)
            }
            Ok(None) => {
                self.release(id);
                self.emit_failed(id, AppError::Cancelled, None, &toast);
                Ok(Dispatched::Done)
            }
            Err(error) => {
                self.release(id);
                self.emit_failed(id, error.clone(), None, &toast);
                Err(error)
            }
        }
    }

    /// Claims the slot for a new recording and returns its task id. A recording already holds it: that one gets a
    /// stop, and `None` comes back.
    fn claim(&self, control: mpsc::UnboundedSender<Control>) -> Option<TaskId> {
        let mut slot = self.slot();
        if let Some(active) = slot.as_ref() {
            let _ = active.control.unbounded_send(Control::Stop);
            return None;
        }
        let id = self.task.next_id();
        *slot = Some(ActiveRecording { id, control });
        Some(id)
    }

    /// Chooses the area, counts down, opens the stream and starts the encoder. `None` means a stop or abort arrived
    /// first, so nothing started.
    async fn begin(
        &self,
        id: TaskId,
        mut commands: mpsc::UnboundedReceiver<Control>,
        area: Area,
        gif: bool,
        ts: TaskSettings,
    ) -> Result<Option<Live>, AppError> {
        let (codec, fps) = if gif { (ts.recording.gif_codec, ts.recording.gif_fps) } else { (ts.recording.codec, ts.recording.fps) };
        // Whether the area is drawn, so a stop while choosing has an overlay of this recording's to close.
        let draws = match area {
            Area::Region => true,
            Area::LastRecording => self.last_recording_area.lock().unwrap_or_else(PoisonError::into_inner).is_none(),
            Area::ActiveWindow | Area::CustomRegion => false,
        };
        let Some(chosen) = unless_stopped(&mut commands, self.choose_area(area, &ts)).await else {
            // Dropping the choice leaves a drawn area's overlay open.
            if draws {
                self.ports.overlay.close();
            }
            return Ok(None);
        };
        let rect = chosen?;
        if stop_requested(&mut commands) {
            return Ok(None);
        }
        for seconds_left in (1..=ts.recording.countdown_secs).rev() {
            self.emit(TaskEvent::RecordingCountdown { id, seconds_left });
            self.ports.clock.sleep(SECOND).await;
            if stop_requested(&mut commands) {
                return Ok(None);
            }
        }
        // The output is prepared before the stream opens, so no error path below drops an open stream.
        let dir = self.settings.get().screenshots_dir_for(&ts);
        let output = self.task.recording_output(&ts, &dir, (rect.width, rect.height), codec.extension()).await?;
        if let Some(folder) = output.parent() {
            files::create_dir_all(self.ports.spawner.as_ref(), folder.to_path_buf())
                .await
                .map_err(|error| AppError::Io(error.to_string()))?;
        }
        let opened = unless_stopped(&mut commands, async {
            let devices = self.audio_devices(&ts).await;
            let input = self.ports.stream.open(rect, ts.recording.show_cursor, fps, &devices).await.map_err(AppError::from)?;
            Ok::<_, AppError>((devices, input))
        })
        .await;
        let Some(opened) = opened else {
            return Ok(None);
        };
        let (devices, input) = opened?;
        if stop_requested(&mut commands) {
            // Closing a stream blocks (PipeWire, the portal), so it is dropped off the async worker.
            self.ports.spawner.run_blocking(Box::new(move || drop(input))).await;
            return Ok(None);
        }
        let spec = RecordSpec {
            area: rect,
            codec,
            fps,
            quality: ts.recording.quality,
            show_cursor: ts.recording.show_cursor,
            audio: devices,
            custom_args: ts.recording.custom_args.clone(),
            camera: self.camera(&ts).await,
        };
        let handle = self.ports.media.start(input, spec, output.clone()).await.map_err(AppError::from)?;
        if stop_requested(&mut commands) {
            // The stop came while the encoder started. It is not dropped midway; it is stopped here, and its file goes.
            let _ = handle.abort().await;
            return Ok(None);
        }
        self.emit(TaskEvent::RecordingStarted { id, area: rect });
        *self.last_recording_area.lock().unwrap_or_else(PoisonError::into_inner) = Some(rect);
        let fixed_secs = ts.recording.fixed_duration_secs;
        Ok(Some(Live { handle, output, commands, ts, fixed_secs }))
    }

    /// The area to record: a drawn one, the active window's rectangle, the custom region, or the last recording's.
    async fn choose_area(&self, area: Area, ts: &TaskSettings) -> Result<PhysRect, AppError> {
        let rect = match area {
            Area::Region => self.drawn_area(ts).await?,
            Area::ActiveWindow => {
                let target = self.ports.hover.active_window().await.map_err(AppError::from)?;
                target.ok_or_else(|| AppError::Unsupported("no active window".to_owned()))?.rect
            }
            Area::CustomRegion => {
                ts.capture.custom_region.ok_or_else(|| AppError::Unsupported("set a custom region in settings".to_owned()))?
            }
            Area::LastRecording => {
                let last = *self.last_recording_area.lock().unwrap_or_else(PoisonError::into_inner);
                match last {
                    Some(rect) => rect,
                    None => self.drawn_area(ts).await?,
                }
            }
        };
        even(rect)
    }

    /// An area the user draws. Several drawn rects give their bounding box.
    async fn drawn_area(&self, ts: &TaskSettings) -> Result<PhysRect, AppError> {
        let frames = self.frames(ts).await?;
        let selection = region::pick(&self.ports, frames, &ts.capture.region).await?;
        selection.rects.into_iter().reduce(PhysRect::union).ok_or_else(|| AppError::Io("no area was selected".to_owned()))
    }

    /// The audio inputs the settings ask for: the microphone (its name from the stream's device list) and the
    /// system sound.
    async fn audio_devices(&self, ts: &TaskSettings) -> Vec<AudioDevice> {
        let mut devices = Vec::new();
        if let Some(id) = ts.recording.microphone.as_deref().filter(|id| !id.is_empty()) {
            let listed = self.ports.stream.audio_devices().await.unwrap_or_default();
            let name = listed.into_iter().find(|device| device.id == id).map_or_else(|| id.to_owned(), |device| device.name);
            devices.push(AudioDevice { id: id.to_owned(), name, loopback: false });
        }
        if ts.recording.record_system_audio {
            devices.push(AudioDevice { id: LOOPBACK.to_owned(), name: LOOPBACK.to_owned(), loopback: true });
        }
        devices
    }

    /// The camera overlay the settings ask for. With no camera named, the first camera the stream
    /// lists is used; a platform that lists none leaves the name empty for its recorder to resolve.
    async fn camera(&self, ts: &TaskSettings) -> Option<laqta_domain::record::CameraOverlay> {
        let mut camera = ts.recording.camera.enabled.then(|| ts.recording.camera.clone())?;
        if camera.device.trim().is_empty()
            && let Ok(listed) = self.ports.stream.camera_devices().await
            && let Some(first) = listed.into_iter().next()
        {
            camera.device = first;
        }
        Some(camera)
    }

    /// Sends a control command to the recording, if one runs. Its driver does the work.
    fn send_control(&self, control: Control) {
        if let Some(active) = self.slot().as_ref() {
            let _ = active.control.unbounded_send(control);
        }
    }

    fn slot(&self) -> MutexGuard<'_, Option<ActiveRecording>> {
        self.recording.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Frees the slot when the recording `id` holds it.
    fn release(&self, id: TaskId) {
        let mut slot = self.slot();
        if slot.as_ref().is_some_and(|active| active.id == id) {
            *slot = None;
        }
    }

    fn emit(&self, event: TaskEvent) {
        self.task.emit(event);
    }

    /// Reports the recording `id` as failed: the event, and a toast unless the user cancelled it. The toast names
    /// the error. A toast that cannot be shown is logged.
    fn emit_failed(&self, id: TaskId, error: AppError, partial: Option<PathBuf>, options: &ToastOptions) {
        if !matches!(error, AppError::Cancelled) {
            let toast = Toast {
                title: "Recording failed".to_owned(),
                body: error.to_string(),
                image: None,
                path: None,
                url: None,
                options: options.clone(),
            };
            if let Err(error) = self.ports.notifier.toast(toast) {
                tracing::warn!(%error, "recording: the failure notice was not shown");
            }
        }
        self.emit(TaskEvent::RecordingFailed { id, error, partial });
    }
}

/// The area with an even width and height: an odd side loses its last pixel. An area with a zero side is an error.
fn even(rect: PhysRect) -> Result<PhysRect, AppError> {
    let rect = PhysRect { width: rect.width & !1, height: rect.height & !1, ..rect };
    if rect.width == 0 || rect.height == 0 {
        return Err(AppError::Unsupported("the recording area is empty".to_owned()));
    }
    Ok(rect)
}

/// A panic payload as text, as the Task module reads it.
fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "panic".to_owned()
    }
}

/// Resolves when a stop or abort arrives, or when the channel closes. Pause and resume are dropped: nothing is
/// recording yet.
async fn wait_for_stop(commands: &mut mpsc::UnboundedReceiver<Control>) {
    while let Some(control) = commands.next().await {
        if matches!(control, Control::Stop | Control::Abort) {
            return;
        }
    }
}

/// Runs `work` unless a stop or abort arrives first. `None` means the stop won, and `work` was dropped (a portal
/// request in `work` goes with it). `commands` is only borrowed, so it works afterwards.
async fn unless_stopped<T>(commands: &mut mpsc::UnboundedReceiver<Control>, work: impl Future<Output = T>) -> Option<T> {
    future::or(async { Some(work.await) }, async {
        wait_for_stop(commands).await;
        None
    })
    .await
}

/// Whether a stop or abort is waiting. Pause and resume are dropped: nothing is recording yet.
fn stop_requested(commands: &mut mpsc::UnboundedReceiver<Control>) -> bool {
    while let Ok(control) = commands.try_recv() {
        if matches!(control, Control::Stop | Control::Abort) {
            return true;
        }
    }
    false
}
