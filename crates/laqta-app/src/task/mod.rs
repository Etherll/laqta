//! The Task module owns the task lifecycle (ADR 011). Submitting a job emits `InQueue` and returns; the job
//! waits for a capture slot, runs its after-capture steps, and reports through `TaskEvent`s. A panic in the
//! job body becomes a `Failed` event (ADR 020). A stop request ends a queued job at once, and a running job
//! at its next checkpoint.

mod encode;
mod run;
mod save;
mod status;
#[cfg(test)]
mod tests;
mod upload;

use std::any::Any;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_broadcast::Sender;
use futures_channel::oneshot;
use futures_lite::FutureExt;
use laqta_domain::workflow::TaskSettings;
use laqta_domain::{Bitmap, Frame};
use laqta_ports::{AppError, BoxFuture, Spawner, TaskEvent, TaskId, TaskStatus};

use crate::dispatcher::{Ports, SettingsCell};
use status::{Jobs, Queue, Reporter, Stop};

/// Capture jobs that may run at once. Further jobs wait in `InQueue`.
const CAPTURE_SLOTS: usize = 2;
/// Choosing a file name and writing it happen one at a time, so two jobs never pick the same name.
const SAVE_SLOTS: usize = 1;

/// The image a Capture job works on.
#[derive(Clone, Debug)]
pub enum CaptureImage {
    /// One frame per monitor. Task composes them.
    Frames(Vec<Frame>),
    /// Already cropped, for example a region.
    Image(Bitmap),
}

/// Facts about the capture that the name patterns read (`%t`, `%pn`).
#[derive(Clone, Debug, Default)]
pub struct CaptureInfo {
    pub window_title: String,
    pub process_name: String,
}

/// What an Upload job sends.
#[derive(Clone, Debug)]
pub enum UploadSource {
    /// Text, uploaded as a `.txt` file.
    Text(String),
    /// A file on disk. Its kind (image, text or other) picks the destination by extension.
    File(PathBuf),
    /// A URL to download, then upload as a file of the downloaded type.
    Url(String),
    /// A URL to shorten. Nothing is uploaded; the after-upload steps run with shortening on.
    ShortenUrl(String),
    /// A file already saved to the screenshots folder (a finished recording). Its after-capture steps run as for
    /// a saved image: copy the file, copy its path, reveal it. Then the upload (when its flag is set), the
    /// after-upload steps and the toast.
    SavedFile(PathBuf),
}

/// Work the dispatcher hands to Task.
#[derive(Clone, Debug)]
pub enum TaskJob {
    /// `settings` are the effective task settings. A `None` `screenshots_dir` means the application's folder.
    Capture { image: CaptureImage, settings: TaskSettings, info: CaptureInfo },
    /// `settings` are the effective task settings, read for the upload and the after-upload steps.
    Upload { source: UploadSource, settings: TaskSettings },
}

/// What the steps of every job share. Each job gets a clone.
#[derive(Clone)]
struct Context {
    ports: Arc<Ports>,
    /// The current settings, read by the steps that use presets and actions (they are not in the job's settings).
    settings: Arc<SettingsCell>,
    reporter: Reporter,
    /// The save slots: choosing a name and writing happen one at a time.
    saves: Arc<Queue>,
    /// The value of `%i` for the next saved file. Starts at 1 and advances once per file written.
    counter: Arc<AtomicU64>,
}

pub(crate) struct Task {
    ports: Arc<Ports>,
    settings: Arc<SettingsCell>,
    reporter: Reporter,
    capture_slots: Arc<Queue>,
    save_slots: Arc<Queue>,
    jobs: Arc<Jobs>,
    counter: Arc<AtomicU64>,
    next_id: AtomicU64,
}

impl Task {
    pub(crate) fn new(ports: Arc<Ports>, settings: Arc<SettingsCell>, events: Sender<TaskEvent>) -> Task {
        Task {
            ports,
            settings,
            reporter: Reporter::new(events),
            capture_slots: Arc::new(Queue::new(CAPTURE_SLOTS)),
            save_slots: Arc::new(Queue::new(SAVE_SLOTS)),
            jobs: Arc::new(Jobs::default()),
            counter: Arc::new(AtomicU64::new(1)),
            next_id: AtomicU64::new(1),
        }
    }

    /// A new task id, for a recording (its events carry the id, but it is not a Task job).
    pub(crate) fn next_id(&self) -> TaskId {
        TaskId(self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    /// Sends an event on the task channel. The recording session reports through it.
    pub(crate) fn emit(&self, event: TaskEvent) {
        self.reporter.emit(event);
    }

    /// The file a recording writes (M4 §4.4): the first free name a saved image would get in `dir`, with `ext`.
    pub(crate) async fn recording_output(
        &self,
        settings: &TaskSettings,
        dir: &Path,
        size: (u32, u32),
        ext: &str,
    ) -> Result<PathBuf, AppError> {
        save::recording_output(&self.context(), settings, dir, size, ext).await
    }

    /// Emits `InQueue` and hands the job to the spawner. The job runs later, unless it is stopped first.
    pub(crate) fn submit(&self, job: TaskJob) -> TaskId {
        let id = self.next_id();
        let job = self.resolve(job);
        self.jobs.add(id);
        self.reporter.status(id, TaskStatus::InQueue);
        self.ports.spawner.spawn(self.job(id, job));
        id
    }

    /// Stops a job. A queued job is reported `Stopped` now and never runs. A running job stops at its next
    /// checkpoint and keeps any file it has already saved. False when the job is unknown or has finished.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn stop(&self, id: TaskId) -> bool {
        match self.jobs.request_stop(id) {
            Stop::Unknown => false,
            Stop::Queued => {
                self.reporter.status(id, TaskStatus::Stopped);
                true
            }
            Stop::Running => true,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn stop_all(&self) {
        for id in self.jobs.ids() {
            self.stop(id);
        }
    }

    /// The folder is resolved here, so the job runs with the folder it was submitted with.
    fn resolve(&self, job: TaskJob) -> TaskJob {
        match job {
            TaskJob::Capture { image, mut settings, info } => {
                settings.screenshots_dir.get_or_insert_with(|| self.settings.get().screenshots_dir.clone());
                TaskJob::Capture { image, settings, info }
            }
            upload @ TaskJob::Upload { .. } => upload,
        }
    }

    /// What the steps of a job share, built from this Task's ports, settings, slots and counter.
    fn context(&self) -> Context {
        Context {
            ports: Arc::clone(&self.ports),
            settings: Arc::clone(&self.settings),
            reporter: self.reporter.clone(),
            saves: Arc::clone(&self.save_slots),
            counter: Arc::clone(&self.counter),
        }
    }

    fn job(&self, id: TaskId, job: TaskJob) -> BoxFuture<'static, ()> {
        let capture_slots = Arc::clone(&self.capture_slots);
        let jobs = Arc::clone(&self.jobs);
        let cx = self.context();
        Box::pin(async move {
            // Stopped while queued: `stop` has already reported it, so it takes no slot and runs nothing.
            if !jobs.contains(id) {
                return;
            }
            // The slot is taken and released outside the guarded body (ADR 020). Upload jobs take a slot too.
            let _slot = capture_slots.acquire().await;
            let Some(stop) = jobs.start(id) else { return };
            let reporter = cx.reporter.clone();
            let body = async move {
                match job {
                    TaskJob::Capture { image, settings, info } => run::capture(cx, id, image, settings, info, stop).await,
                    TaskJob::Upload { source, settings } => upload::run(cx, id, source, settings, stop).await,
                }
            };
            if let Err(payload) = AssertUnwindSafe(body).catch_unwind().await {
                reporter.fail(id, AppError::Panic(panic_message(&*payload)));
            }
            jobs.finish(id);
        })
    }
}

/// Runs `work` on the compute pool and returns its result. A panic comes back as `AppError::Panic`,
/// because the pool may drop the work instead of reporting it (ADR 020).
pub(crate) async fn compute<T: Send + 'static>(
    spawner: &dyn Spawner,
    work: impl FnOnce() -> Result<T, AppError> + Send + 'static,
) -> Result<T, AppError> {
    let (tx, rx) = oneshot::channel();
    spawner
        .run_compute(Box::new(move || {
            let outcome = std::panic::catch_unwind(AssertUnwindSafe(work));
            let _ = tx.send(outcome);
        }))
        .await;
    match rx.await {
        Ok(Ok(result)) => result,
        Ok(Err(payload)) => Err(AppError::Panic(panic_message(&*payload))),
        Err(oneshot::Canceled) => Err(AppError::Cancelled),
    }
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "panic".to_owned()
    }
}
