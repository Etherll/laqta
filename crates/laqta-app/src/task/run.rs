//! The steps of a Capture job, in order (M1 §4.3, M2 §4, ADR 011). Each step runs only when its flag in
//! `settings.after_capture` is set. A stop request is checked before each step. A step that names a file
//! is skipped quietly when this job saved none. A failed step is reported as `StepFailed` and the job goes
//! on, except for the pixel work, the save and the upload, whose failure ends the job as `Failed`. The upload
//! phase comes after the local steps and before the toast, so the toast can carry the URL (upload.rs).

use std::fmt::Display;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use laqta_domain::Bitmap;
use laqta_domain::workflow::{AfterCaptureTasks, TaskSettings};
use laqta_ports::{AppError, TaskEvent, TaskId, TaskStatus, Toast, UploadData, UploadKind, UploadRequest};

use super::encode::{self, Encoded, Plan, Prepared};
use super::save::{self, Outcome, SavedFile};
use super::upload::{self, Ended, Sent};
use super::{CaptureImage, CaptureInfo, Context, compute};
use crate::files;

/// `Err` means the rest of the job does not run. Its stop or failure is already reported.
pub(super) type Step<T = ()> = Result<T, Halt>;

#[derive(Debug)]
pub(super) struct Halt;

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

pub(super) struct Job {
    pub(super) cx: Context,
    pub(super) id: TaskId,
    pub(super) settings: TaskSettings,
    pub(super) info: CaptureInfo,
    pub(super) stop: Arc<AtomicBool>,
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
        let general = &self.settings.general;
        if general.play_sound_after_capture {
            let _ = self.cx.ports.notifier.sound(laqta_ports::Sound::Capture, general.custom_capture_sound.as_deref());
        }
        let size = (image.width(), image.height());
        // The upload sends the encoded bytes as they are, so the image is encoded once for both. The copy is
        // made only when an upload needs it.
        let for_upload = if flags.contains(AfterCaptureTasks::UPLOAD_IMAGE_TO_HOST) { encoded.clone() } else { None };

        let saved_file = match encoded {
            Some(encoded) if flags.contains(AfterCaptureTasks::SAVE_IMAGE_TO_FILE) => self.save_image(size, encoded).await?,
            _ => None,
        };
        let saved = saved_file.as_ref();

        if flags.contains(AfterCaptureTasks::COPY_IMAGE_TO_CLIPBOARD) {
            self.copy_image(&image)?;
        }
        let mut thumbnail_path = None;
        if flags.contains(AfterCaptureTasks::SAVE_THUMBNAIL_IMAGE_TO_FILE) {
            thumbnail_path = self.save_thumbnail(saved, thumbnail).await?;
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

        // The upload phase. A failure or a stop keeps the saved file: nothing is deleted on that path.
        let url = match for_upload {
            Some(encoded) => {
                let bytes = upload::byte_len(encoded.bytes.len());
                let name = match saved {
                    Some(saved) => upload::file_name_of(&saved.path),
                    None => self.generated_name(encoded.format.extension(), Some(size)),
                };
                let request = UploadRequest {
                    file_name: name.clone(),
                    kind: UploadKind::Image,
                    data: UploadData::Bytes(encoded.bytes),
                    mime: upload::mime_for_name(&name).to_owned(),
                };
                let sent = Sent { path: saved.map(|saved| saved.path.clone()), name, thumbnail: thumbnail_path };
                match self.send(UploadKind::Image, request, bytes, sent, self.settings.after_upload).await {
                    Ok(url) => url,
                    Err(Ended::Stopped) => return Err(Halt),
                    Err(Ended::Failed(error)) => return Err(self.fail(error)),
                }
            }
            None => None,
        };

        if let Some(preview) = preview {
            self.toast(saved, preview, url)?;
        }
        if flags.contains(AfterCaptureTasks::DELETE_FILE) {
            self.delete(saved).await?;
        }
        self.complete();
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

    /// Writes the thumbnail beside the saved image. Returns its path when it was written.
    async fn save_thumbnail(&self, saved: Option<&SavedFile>, made: Option<Result<Encoded, AppError>>) -> Step<Option<PathBuf>> {
        let (Some(saved), Some(made)) = (saved, made) else { return Ok(None) };
        self.checkpoint()?;
        let result = match made {
            Ok(encoded) => save::thumbnail(&self.cx, saved, &self.settings.image.thumbnail_suffix, encoded).await,
            Err(error) => Err(error),
        };
        match result {
            Ok(path) => {
                self.emit(TaskEvent::ThumbnailSaved { id: self.id, path: path.clone() });
                Ok(Some(path))
            }
            Err(error) => {
                self.step_failed("thumbnail", error);
                Ok(None)
            }
        }
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

    /// The toast for a capture. `url` is the upload's URL, shortened when shortening worked.
    fn toast(&self, saved: Option<&SavedFile>, preview: Bitmap, url: Option<String>) -> Step {
        self.checkpoint()?;
        let (title, body, path) = match saved {
            Some(saved) => ("Screenshot saved", saved.path.display().to_string(), Some(saved.path.clone())),
            None => ("Screenshot copied", String::new(), None),
        };
        let toast = Toast { title: title.to_owned(), body, image: Some(preview), path, url, options: self.settings.general.toast.clone() };
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
    pub(super) fn checkpoint(&self) -> Step {
        if !self.stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        self.cx.reporter.status(self.id, TaskStatus::Stopping);
        self.cx.reporter.status(self.id, TaskStatus::Stopped);
        Err(Halt)
    }

    /// Reports a failure that ends the job.
    pub(super) fn fail(&self, error: AppError) -> Halt {
        let general = &self.settings.general;
        if general.play_sound_after_task {
            let _ = self.cx.ports.notifier.sound(laqta_ports::Sound::Error, general.custom_error_sound.as_deref());
        }
        self.cx.reporter.fail(self.id, error);
        Halt
    }

    /// The task-completed sound, then `Completed`. Only a job that ran to its last step gets here.
    pub(super) fn complete(&self) {
        let general = &self.settings.general;
        if general.play_sound_after_task {
            let _ = self.cx.ports.notifier.sound(laqta_ports::Sound::TaskCompleted, general.custom_task_sound.as_deref());
        }
        self.cx.reporter.status(self.id, TaskStatus::Completed);
    }

    pub(super) fn step_failed(&self, step: &'static str, error: impl Display) {
        self.emit(TaskEvent::StepFailed { id: self.id, step, reason: error.to_string() });
    }

    pub(super) fn emit(&self, event: TaskEvent) {
        self.cx.reporter.emit(event);
    }
}

/// Whether any step reads a file, or uploads the image, so the image is encoded once for all of them.
fn needs_file(flags: AfterCaptureTasks) -> bool {
    flags.intersects(
        AfterCaptureTasks::SAVE_IMAGE_TO_FILE
            | AfterCaptureTasks::SAVE_THUMBNAIL_IMAGE_TO_FILE
            | AfterCaptureTasks::COPY_FILE_TO_CLIPBOARD
            | AfterCaptureTasks::COPY_FILE_PATH_TO_CLIPBOARD
            | AfterCaptureTasks::SHOW_IN_EXPLORER
            | AfterCaptureTasks::UPLOAD_IMAGE_TO_HOST
            | AfterCaptureTasks::DELETE_FILE,
    )
}
