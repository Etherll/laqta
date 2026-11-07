//! The steps of a Capture job, in order (M1 §4.3, ADR 011). Each step runs only when its flag in
//! `settings.after_capture` is set. A stop request is checked before each step. A step that names a file
//! is skipped quietly when this job saved none. A failed step is reported as `StepFailed` and the job goes
//! on, except for the pixel work and the save, whose failure ends the job as `Failed`.

use std::fmt::Display;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use laqta_domain::workflow::{AfterCaptureTasks, TaskSettings};
use laqta_domain::Bitmap;
use laqta_ports::{AppError, TaskEvent, TaskId, TaskStatus, Toast};

use super::encode::{self, Encoded, Plan, Prepared};
use super::save::{self, Outcome, SavedFile};
use super::{compute, CaptureImage, CaptureInfo, Context};
use crate::files;

/// `Err` means the rest of the job does not run. Its stop or failure is already reported.
type Step<T = ()> = Result<T, Halt>;

#[derive(Debug)]
struct Halt;

/// Runs one Capture job from `Preparing` to its last step.
pub(super) async fn capture(
    cx: Context,
    id: TaskId,
    input: CaptureImage,
    settings: TaskSettings,
    info: CaptureInfo,
    stop: Arc<AtomicBool>,
) {
    cx.reporter.status(id, TaskStatus::Preparing);
    cx.reporter.status(id, TaskStatus::Working);
    let job = Job { cx, id, settings, info, stop };
    // The outcome is already reported, so there is nothing to do with it here.
    let _ = job.run(input).await;
}

struct Job {
    cx: Context,
    id: TaskId,
    settings: TaskSettings,
    info: CaptureInfo,
    stop: Arc<AtomicBool>,
}

impl Job {
    async fn run(&self, input: CaptureImage) -> Step {
        let flags = self.settings.after_capture;
        let plan = Plan {
            encode: needs_file(flags),
            thumbnail: flags.contains(AfterCaptureTasks::SAVE_IMAGE_TO_FILE)
                && flags.contains(AfterCaptureTasks::SAVE_THUMBNAIL_IMAGE_TO_FILE),
            preview: self.settings.general.show_toast,
        };
        let Prepared { image, encoded, thumbnail, preview } = self.prepare(input, plan).await?;
        let size = (image.width(), image.height());

        let saved_file = match encoded {
            Some(encoded) if flags.contains(AfterCaptureTasks::SAVE_IMAGE_TO_FILE) => {
                self.save_image(size, encoded).await?
            }
            _ => None,
        };
        let saved = saved_file.as_ref();

        if flags.contains(AfterCaptureTasks::COPY_IMAGE_TO_CLIPBOARD) {
            self.copy_image(&image)?;
        }
        if flags.contains(AfterCaptureTasks::SAVE_THUMBNAIL_IMAGE_TO_FILE) {
            self.save_thumbnail(saved, thumbnail).await?;
        }
        if flags.contains(AfterCaptureTasks::COPY_FILE_TO_CLIPBOARD) {
            self.copy_file(saved)?;
        }
        if flags.contains(AfterCaptureTasks::COPY_FILE_PATH_TO_CLIPBOARD) {
            self.copy_path(saved)?;
        }
        if flags.contains(AfterCaptureTasks::SHOW_IN_EXPLORER) {
            self.reveal(saved)?;
        }
        if let Some(preview) = preview {
            self.toast(saved, preview)?;
        }
        if flags.contains(AfterCaptureTasks::DELETE_FILE) {
            self.delete(saved).await?;
        }
        self.cx.reporter.status(self.id, TaskStatus::Completed);
        Ok(())
    }

    /// Composes and encodes on the compute pool, in one hop.
    async fn prepare(&self, input: CaptureImage, plan: Plan) -> Step<Prepared> {
        self.checkpoint()?;
        let ports = Arc::clone(&self.cx.ports);
        let image_settings = self.settings.image.clone();
        let work = move || encode::prepare(&ports.codecs, input, plan, &image_settings);
        compute(self.cx.ports.spawner.as_ref(), work).await.map_err(|error| self.fail(error))
    }

    async fn save_image(&self, size: (u32, u32), encoded: Encoded) -> Step<Option<SavedFile>> {
        self.checkpoint()?;
        match save::image(&self.cx, &self.settings, &self.info, size, encoded).await {
            Ok(Outcome::Saved(saved)) => {
                self.emit(TaskEvent::Saved { id: self.id, path: saved.path.clone() });
                Ok(Some(saved))
            }
            Ok(Outcome::Skipped(path)) => {
                self.emit(TaskEvent::SaveSkipped { id: self.id, path });
                Ok(None)
            }
            Err(error) => Err(self.fail(error)),
        }
    }

    fn copy_image(&self, image: &Bitmap) -> Step {
        self.checkpoint()?;
        match self.cx.ports.clipboard.set_image(image) {
            Ok(()) => self.emit(TaskEvent::Copied { id: self.id }),
            Err(error) => self.emit(TaskEvent::CopyFailed { id: self.id, reason: error.to_string() }),
        }
        Ok(())
    }

    async fn save_thumbnail(&self, saved: Option<&SavedFile>, made: Option<Result<Encoded, AppError>>) -> Step {
        let (Some(saved), Some(made)) = (saved, made) else { return Ok(()) };
        self.checkpoint()?;
        let result = match made {
            Ok(encoded) => save::thumbnail(&self.cx, saved, &self.settings.image.thumbnail_suffix, encoded).await,
            Err(error) => Err(error),
        };
        match result {
            Ok(path) => self.emit(TaskEvent::ThumbnailSaved { id: self.id, path }),
            Err(error) => self.step_failed("thumbnail", error),
        }
        Ok(())
    }

    fn copy_file(&self, saved: Option<&SavedFile>) -> Step {
        let Some(saved) = saved else { return Ok(()) };
        self.checkpoint()?;
        match self.cx.ports.clipboard.set_files(std::slice::from_ref(&saved.path)) {
            Ok(()) => self.emit(TaskEvent::FileCopied { id: self.id }),
            Err(error) => self.step_failed("copy_file", error),
        }
        Ok(())
    }

    fn copy_path(&self, saved: Option<&SavedFile>) -> Step {
        let Some(saved) = saved else { return Ok(()) };
        self.checkpoint()?;
        match self.cx.ports.clipboard.set_text(&saved.path.display().to_string()) {
            Ok(()) => self.emit(TaskEvent::PathCopied { id: self.id, path: saved.path.clone() }),
            Err(error) => self.step_failed("copy_path", error),
        }
        Ok(())
    }

    fn reveal(&self, saved: Option<&SavedFile>) -> Step {
        let Some(saved) = saved else { return Ok(()) };
        self.checkpoint()?;
        match self.cx.ports.shell.reveal(&saved.path) {
            Ok(()) => self.emit(TaskEvent::Revealed { id: self.id, path: saved.path.clone() }),
            Err(error) => self.step_failed("reveal", error),
        }
        Ok(())
    }

    fn toast(&self, saved: Option<&SavedFile>, preview: Bitmap) -> Step {
        self.checkpoint()?;
        let (title, body, path) = match saved {
            Some(saved) => ("Screenshot saved", saved.path.display().to_string(), Some(saved.path.clone())),
            None => ("Screenshot copied", String::new(), None),
        };
        let toast = Toast {
            title: title.to_owned(),
            body,
            image: Some(preview),
            path,
            url: None,
            options: self.settings.general.toast.clone(),
        };
        match self.cx.ports.notifier.toast(toast) {
            Ok(()) => self.emit(TaskEvent::Notified { id: self.id }),
            Err(error) => self.step_failed("toast", error),
        }
        Ok(())
    }

    async fn delete(&self, saved: Option<&SavedFile>) -> Step {
        let Some(saved) = saved else { return Ok(()) };
        self.checkpoint()?;
        match files::remove(self.cx.ports.spawner.as_ref(), saved.path.clone()).await {
            Ok(()) => self.emit(TaskEvent::Deleted { id: self.id, path: saved.path.clone() }),
            Err(error) => self.step_failed("delete", error),
        }
        Ok(())
    }

    /// Stops the job when a stop was requested. Emits `Stopping`, then `Stopped`, and leaves any saved file.
    fn checkpoint(&self) -> Step {
        if !self.stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        self.cx.reporter.status(self.id, TaskStatus::Stopping);
        self.cx.reporter.status(self.id, TaskStatus::Stopped);
        Err(Halt)
    }

    /// Reports a failure that ends the job.
    fn fail(&self, error: AppError) -> Halt {
        self.cx.reporter.fail(self.id, error);
        Halt
    }

    fn step_failed(&self, step: &'static str, error: impl Display) {
        self.emit(TaskEvent::StepFailed { id: self.id, step, reason: error.to_string() });
    }

    fn emit(&self, event: TaskEvent) {
        self.cx.reporter.emit(event);
    }
}

/// Whether any step reads a file, so the image is encoded once for all of them.
fn needs_file(flags: AfterCaptureTasks) -> bool {
    flags.intersects(
        AfterCaptureTasks::SAVE_IMAGE_TO_FILE
            | AfterCaptureTasks::SAVE_THUMBNAIL_IMAGE_TO_FILE
            | AfterCaptureTasks::COPY_FILE_TO_CLIPBOARD
            | AfterCaptureTasks::COPY_FILE_PATH_TO_CLIPBOARD
            | AfterCaptureTasks::SHOW_IN_EXPLORER
            | AfterCaptureTasks::DELETE_FILE,
    )
}
