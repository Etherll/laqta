//! The steps of a Capture job, in order (ADR 011). Each step runs only when its flag in
//! `settings.after_capture` is set. A stop request is checked before each step. A step that names a file
//! is skipped, with a `StepFailed` that says why, when this job saved none. A failed step is reported as
//! `StepFailed` and the job goes on, except for the pixel work and the upload, whose failure ends the job as
//! `Failed`. A failed save is a step failure too: the copy, the pin and the upload still run, and the toast
//! reports the save error. A prompt the
//! user cancels stops the job as `Stopped`. The order is the quick task menu, the after-capture window, the
//! annotation, the effects, the beautifier, the encode and the save, the actions, the copies, the upload (with
//! the before-upload window), and the toast, so the toast can carry the URL (upload.rs).

use std::fmt::Display;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures_channel::oneshot;
use futures_lite::FutureExt;
use laqta_domain::Bitmap;
use laqta_domain::effects::Preset;
use laqta_domain::workflow::{AfterCaptureTasks, AfterUploadTasks, TaskSettings};
use laqta_ports::{AppError, Clipboard, PortError, TaskEvent, TaskId, TaskStatus, Toast, UploadData, UploadKind, UploadRequest};

use super::encode::{self, Encoded, Plan, Prepared};
use super::save::{self, Outcome, SavedFile};
use super::upload::{self, Ended, Sent};
use super::{CaptureImage, CaptureInfo, Context, compute};
use crate::files;

/// `Err` means the rest of the job does not run. Its stop or failure is already reported.
pub(super) type Step<T = ()> = Result<T, Halt>;

#[derive(Debug)]
pub(super) struct Halt;

/// The quick task menu's choices, in the order the menu shows them. The last one cancels the job.
const QUICK_MENU: [&str; 7] = ["Save and copy", "Save", "Copy", "Upload", "Save and upload", "Annotate", "Cancel"];

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
    let mut job = Job { cx, id, settings, info, stop, file: None, upload_sound: Default::default(), save_error: None };
    job.read_random_lines().await;
    // The outcome is already reported, so there is nothing to do with it here. A panic is not: it ends the job as a
    // failure that the caller reports (ADR 020), but the file still goes first.
    if let Err(payload) = AssertUnwindSafe(job.run(input)).catch_unwind().await {
        job.finally().await;
        std::panic::resume_unwind(payload);
    }
}

impl Job {
    /// Reads the files the job's name patterns draw `%rf{path}` lines from, once, before any name is made.
    pub(super) async fn read_random_lines(&mut self) {
        self.info.random_lines = super::save::random_lines(&self.cx, &self.settings).await;
    }
}

/// How long a temporary print file stays for the print handler to read it.
const PRINT_FILE_LIFETIME: std::time::Duration = std::time::Duration::from_secs(300);

pub(super) struct Job {
    pub(super) cx: Context,
    pub(super) id: TaskId,
    pub(super) settings: TaskSettings,
    pub(super) info: CaptureInfo,
    pub(super) stop: Arc<AtomicBool>,
    /// The file this job wrote and later steps use. `DELETE_FILE` removes it when the job ends, whatever the outcome.
    pub(super) file: Option<PathBuf>,
    /// An upload already played the task-completed sound (`play_sound_after_upload`), so the end of
    /// the job doesn't play it again.
    pub(super) upload_sound: std::sync::atomic::AtomicBool,
    /// Why the save failed, when it did. The toast reports it, since there is no saved file to name.
    pub(super) save_error: Option<String>,
}

impl Job {
    /// The steps, then the delete (`DELETE_FILE`), then `Completed` for a job that ran to its last step. The
    /// delete comes before `Completed`, and after the `Failed` or `Stopped` that a halted job has already reported.
    async fn run(&mut self, input: CaptureImage) -> Step {
        let outcome = self.steps(input).await;
        self.finally().await;
        if outcome.is_ok() {
            self.complete();
        }
        outcome
    }

    async fn steps(&mut self, input: CaptureImage) -> Step {
        self.quick_task_menu().await?;
        let input = if needs_pixels(self.settings.after_capture) {
            let composed = self.compose(input).await?;
            CaptureImage::Image(self.edit(composed).await?)
        } else {
            input
        };
        let flags = self.settings.after_capture;
        let plan = Plan {
            encode: needs_file(flags),
            thumbnail: flags.contains(AfterCaptureTasks::SAVE_IMAGE_TO_FILE)
                && flags.contains(AfterCaptureTasks::SAVE_THUMBNAIL_IMAGE_TO_FILE),
            preview: self.settings.general.show_toast,
        };
        let Prepared { image, encoded, thumbnail, preview, warning } = self.prepare(input, plan).await?;
        if let Some(warning) = warning {
            self.step_failed("encode", warning);
        }
        if self.settings.general.play_sound_after_capture {
            let general = &self.settings.general;
            let _ = self.cx.ports.notifier.sound(laqta_ports::Sound::Capture, general.custom_capture_sound.as_deref());
        }
        let size = (image.width(), image.height());
        // The upload sends the encoded bytes as they are, so the image is encoded once for all of them. Copies are
        // made only for the steps that need one.
        let for_upload = if flags.contains(AfterCaptureTasks::UPLOAD_IMAGE_TO_HOST) { encoded.clone() } else { None };
        let for_dialog = if flags.contains(AfterCaptureTasks::SAVE_IMAGE_TO_FILE_WITH_DIALOG) { encoded.clone() } else { None };
        // Printing needs a file; without a saved one it prints a temporary copy, as ShareX prints the image.
        let for_print = if flags.contains(AfterCaptureTasks::SEND_IMAGE_TO_PRINTER) { encoded.clone() } else { None };

        let mut saved_file = match encoded {
            Some(encoded) if flags.contains(AfterCaptureTasks::SAVE_IMAGE_TO_FILE) => self.save_image(size, encoded).await?,
            _ => None,
        };
        if let Some(encoded) = for_dialog {
            let chosen = self.save_with_dialog(size, encoded).await?;
            // With both saves set, both files are written. The plain save is the file the later steps use.
            saved_file = saved_file.or(chosen);
        }
        self.file = saved_file.as_ref().map(|saved| saved.path.clone());
        // An action that writes an output file hands it on: the later steps, and the upload, use that file.
        let mut uploaded_output = None;
        if flags.contains(AfterCaptureTasks::PERFORM_ACTIONS) {
            let saved_path = saved_file.as_ref().map(|saved| saved.path.clone());
            match saved_path {
                Some(path) => {
                    if let Some(output) = self.perform_actions(&path).await? {
                        saved_file = Some(SavedFile { path: output.clone() });
                        self.file = Some(output.clone());
                        uploaded_output = Some(output);
                    }
                }
                None => self.step_failed("action", self.no_file_reason()),
            }
        }
        let saved = saved_file.as_ref();

        if flags.contains(AfterCaptureTasks::COPY_IMAGE_TO_CLIPBOARD) {
            self.copy_image(&image).await?;
        }
        let mut thumbnail_path = None;
        if flags.contains(AfterCaptureTasks::SAVE_THUMBNAIL_IMAGE_TO_FILE) {
            thumbnail_path = self.save_thumbnail(saved, thumbnail).await?;
        }
        // The file, its path or its folder goes on the clipboard, never more than one: file wins over path, path over folder.
        if flags.contains(AfterCaptureTasks::COPY_FILE_TO_CLIPBOARD) {
            self.copy_file(saved).await?;
        } else if flags.contains(AfterCaptureTasks::COPY_FILE_PATH_TO_CLIPBOARD) {
            self.copy_path(saved).await?;
        } else if flags.contains(AfterCaptureTasks::COPY_FOLDER_PATH_TO_CLIPBOARD) {
            self.copy_folder(saved).await?;
        }
        if flags.contains(AfterCaptureTasks::SEND_IMAGE_TO_PRINTER) {
            self.print(saved, for_print).await?;
        }
        if flags.contains(AfterCaptureTasks::SHOW_IN_EXPLORER) {
            self.reveal(saved)?;
        }
        if flags.contains(AfterCaptureTasks::PIN_TO_SCREEN) {
            self.pin(&image)?;
        }
        if flags.contains(AfterCaptureTasks::OPEN_IN_PHOTO_EDITOR) {
            self.checkpoint()?;
            match self.cx.ports.tools.photo_editor(image.clone()) {
                Ok(()) => self.emit(TaskEvent::PhotoEditorOpened { id: self.id }),
                Err(error) => self.step_failed("photo_editor", error),
            }
        }
        if flags.contains(AfterCaptureTasks::SCAN_QR_CODE) {
            self.scan_qr(&image).await?;
        }
        if flags.contains(AfterCaptureTasks::DO_OCR) {
            self.ocr(&image).await?;
        }
        if flags.contains(AfterCaptureTasks::ANALYZE_IMAGE) {
            self.analyze(&image).await?;
        }

        // The upload phase. A failure or a stop ends the job here, and `finally` still deletes the file.
        let url = match for_upload {
            Some(encoded) => self.upload_image(encoded, size, saved, uploaded_output, thumbnail_path).await?,
            None => None,
        };

        if let Some(preview) = preview {
            self.toast(saved, Some(preview), url)?;
        }
        Ok(())
    }

    /// The quick task menu (`SHOW_QUICK_TASK_MENU`). The choice replaces the job's flags. Cancel stops the job.
    async fn quick_task_menu(&mut self) -> Step {
        if !self.settings.after_capture.contains(AfterCaptureTasks::SHOW_QUICK_TASK_MENU) {
            return Ok(());
        }
        self.checkpoint()?;
        let options: Vec<String> = QUICK_MENU.iter().map(|label| (*label).to_owned()).collect();
        let answer = self.cx.ports.prompt.choose("What to do with this capture?", &options).await;
        match answer.ok().and_then(quick_menu_flags) {
            Some(flags) => {
                self.settings.after_capture = flags;
                Ok(())
            }
            None => Err(self.cancelled()),
        }
    }

    /// The steps that work on the composed image, in order: the after-capture window, then the beautifier,
    /// effects and annotation, so what the user draws in the editor is the final image. Each reads the flags
    /// when it is reached, so the window's answer counts.
    async fn edit(&mut self, mut image: Bitmap) -> Step<Bitmap> {
        if self.settings.after_capture.contains(AfterCaptureTasks::SHOW_AFTER_CAPTURE_WINDOW) {
            self.after_capture_window(&image).await?;
        }
        // Background removal comes first, so the beautifier, effects and annotation see the cut-out.
        if self.settings.after_capture.contains(AfterCaptureTasks::REMOVE_BACKGROUND) {
            self.checkpoint()?;
            image = self.remove_background(image).await;
        }
        if self.settings.after_capture.contains(AfterCaptureTasks::BEAUTIFY_IMAGE) {
            self.checkpoint()?;
            image = self.beautify(image).await;
        }
        if self.settings.after_capture.contains(AfterCaptureTasks::ADD_IMAGE_EFFECTS) {
            self.checkpoint()?;
            image = self.effects(image).await;
        }
        if self.settings.after_capture.contains(AfterCaptureTasks::ANNOTATE_IMAGE) {
            self.checkpoint()?;
            image = self.annotate(image).await;
        }
        Ok(image)
    }

    /// The after-capture window. Its answer replaces the job's capture flags and after-upload flags. Cancel stops
    /// the job; a window that cannot be shown is a step failure, and the flags stay as they were.
    async fn after_capture_window(&mut self, image: &Bitmap) -> Step {
        self.checkpoint()?;
        let flags = self.settings.after_capture.bits();
        let after = self.settings.after_upload.bits();
        let answer = self.cx.ports.prompt.after_capture(image, flags, after).await;
        match answer {
            Ok((flags, after)) => {
                self.settings.after_capture = AfterCaptureTasks::from_bits_truncate(flags);
                self.settings.after_upload = AfterUploadTasks::from_bits_truncate(after);
                Ok(())
            }
            Err(PortError::Cancelled) => Err(self.cancelled()),
            Err(error) => {
                self.step_failed("after_capture", error);
                Ok(())
            }
        }
    }

    /// The annotation editor. The image it returns replaces the capture. A closed editor keeps the original.
    async fn annotate(&self, image: Bitmap) -> Bitmap {
        let kept = image.clone();
        let answer = self.cx.ports.editor.edit(image).await;
        match answer {
            Ok(edited) => edited,
            Err(PortError::Cancelled) => kept,
            Err(error) => {
                self.step_failed("annotate", error);
                kept
            }
        }
    }

    /// The effect preset named by the task settings (the first one when the name is empty), applied on the compute
    /// pool. With no such preset the step fails and the image is kept.
    async fn effects(&self, image: Bitmap) -> Bitmap {
        let preset = {
            let settings = self.cx.settings.get();
            let roll = self.cx.ports.rng.next_u64();
            pick_preset(&settings.effect_presets, &self.settings.effect_preset, roll).cloned()
        };
        let Some(preset) = preset else {
            let name = &self.settings.effect_preset;
            let reason =
                if name.is_empty() { "there is no effect preset".to_owned() } else { format!("no effect preset named \"{name}\"") };
            self.step_failed("effects", reason);
            return image;
        };
        let kept = image.clone();
        match self.cx.effects.apply(&self.cx.ports, image, preset).await {
            Ok(out) => out,
            Err(error) => {
                self.step_failed("effects", error);
                kept
            }
        }
    }

    /// The beautifier, on the compute pool.
    async fn beautify(&self, image: Bitmap) -> Bitmap {
        let kept = image.clone();
        let settings = self.settings.beautify.clone();
        let work = move || -> Result<Bitmap, AppError> { Ok(laqta_imaging::beautify::beautify(&image, &settings)) };
        match compute(self.cx.ports.spawner.as_ref(), work).await {
            Ok(out) => out,
            Err(error) => {
                self.step_failed("beautify", error);
                kept
            }
        }
    }

    /// Composes the frames of a capture into one image on the compute pool. An image passes through unchanged.
    async fn compose(&self, input: CaptureImage) -> Step<Bitmap> {
        self.checkpoint()?;
        let work = move || -> Result<Bitmap, AppError> {
            Ok(match input {
                CaptureImage::Frames(frames) => laqta_imaging::compose_monitors(&frames),
                CaptureImage::Image(image) => image,
            })
        };
        compute(self.cx.ports.spawner.as_ref(), work).await.map_err(|error| self.fail(error))
    }

    /// Composes and encodes on the compute pool, in one hop.
    async fn prepare(&self, input: CaptureImage, plan: Plan) -> Step<Prepared> {
        self.checkpoint()?;
        let ports = Arc::clone(&self.cx.ports);
        let image_settings = self.settings.image.clone();
        let work = move || encode::prepare(&ports.codecs, input, plan, &image_settings);
        compute(self.cx.ports.spawner.as_ref(), work).await.map_err(|error| self.fail(error))
    }

    async fn save_image(&mut self, size: (u32, u32), encoded: Encoded) -> Step<Option<SavedFile>> {
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
            Err(error) => {
                // The capture is still in memory: report the failure and let the copy, pin and upload go on.
                self.step_failed("save", &error);
                self.save_error = Some(error.to_string());
                Ok(None)
            }
        }
    }

    /// Why a step that needs a saved file has none: Save is off, or the save did not happen.
    fn no_file_reason(&self) -> &'static str {
        let flags = self.settings.after_capture;
        if flags.intersects(AfterCaptureTasks::SAVE_IMAGE_TO_FILE | AfterCaptureTasks::SAVE_IMAGE_TO_FILE_WITH_DIALOG) {
            "the image was not saved"
        } else {
            "needs a saved file, and saving is off"
        }
    }

    /// Asks where to save the image, then writes it there. Cancel saves nothing and the job goes on. The dialog
    /// is shown before the save slot is taken, so it does not hold up other saves.
    async fn save_with_dialog(&self, size: (u32, u32), encoded: Encoded) -> Step<Option<SavedFile>> {
        self.checkpoint()?;
        let dir = self.settings.screenshots_dir.clone().unwrap_or_default();
        let suggested = dir.join(self.generated_name(encoded.format.extension(), Some(size)));
        let answer = self.cx.ports.prompt.save_path(&suggested).await;
        let path = match answer {
            Ok(path) => path,
            Err(PortError::Cancelled) => return Ok(None),
            Err(error) => {
                self.step_failed("save_dialog", error);
                return Ok(None);
            }
        };
        match save::chosen(&self.cx, path.clone(), encoded.bytes).await {
            Ok(()) => {
                self.emit(TaskEvent::Saved { id: self.id, path: path.clone() });
                Ok(Some(SavedFile { path }))
            }
            Err(error) => {
                self.step_failed("save_dialog", error);
                Ok(None)
            }
        }
    }

    /// Runs the enabled actions that apply to the saved file, in order. An action that writes an
    /// output file hands it to the next action. Returns the output file the chain ended on, when it replaced the
    /// saved file. A non-zero exit, or a program that cannot run, is a step failure.
    async fn perform_actions(&self, saved: &Path) -> Step<Option<PathBuf>> {
        self.checkpoint()?;
        let settings = self.cx.settings.get();
        let mut current: Option<PathBuf> = None;
        for action in settings.actions.iter() {
            let input = current.clone().unwrap_or_else(|| saved.to_path_buf());
            if !action.applies_to(&input) {
                continue;
            }
            self.checkpoint()?;
            let (argv, output) = action.argv(&input);
            let answer = self.cx.ports.shell.run(&action.program, &argv, action.hidden).await;
            match answer {
                Ok(0) => {
                    if let Some(output) = output
                        && files::exists(self.cx.ports.spawner.as_ref(), output.clone()).await
                    {
                        current = Some(output);
                    }
                }
                Ok(code) => self.step_failed("action", format!("{} exited with code {code}", action.name)),
                Err(error) => self.step_failed("action", format!("{}: {error}", action.name)),
            }
        }
        Ok(current)
    }

    /// The image upload. The before-upload window comes first: Cancel skips the upload. The upload sends the
    /// action's output when an action replaced the saved file, and the encoded bytes otherwise.
    async fn upload_image(
        &mut self,
        encoded: Encoded,
        size: (u32, u32),
        saved: Option<&SavedFile>,
        output: Option<PathBuf>,
        thumbnail: Option<PathBuf>,
    ) -> Step<Option<String>> {
        let name = match (&output, saved) {
            (Some(path), _) => upload::file_name_of(path),
            (None, Some(saved)) => upload::file_name_of(&saved.path),
            (None, None) => self.generated_name(encoded.format.extension(), Some(size)),
        };
        // An output file goes to the destination for its own kind (an image, text or other file), like any file upload.
        let kind = if output.is_some() { upload::kind_for_name(&name) } else { UploadKind::Image };
        if self.settings.after_capture.contains(AfterCaptureTasks::SHOW_BEFORE_UPLOAD_WINDOW)
            && !self.choose_destination(kind.clone(), &name).await?
        {
            return Ok(None);
        }
        let (data, bytes) = match output {
            Some(path) => {
                let answer = files::size(self.cx.ports.spawner.as_ref(), path.clone()).await;
                let bytes = answer.map_err(|error| self.fail(AppError::Io(error.to_string())))?;
                (UploadData::Path(path), bytes)
            }
            None => {
                let bytes = upload::byte_len(encoded.bytes.len());
                (UploadData::Bytes(encoded.bytes), bytes)
            }
        };
        let request = UploadRequest { file_name: name.clone(), kind: kind.clone(), data, mime: upload::mime_for_name(&name).to_owned() };
        let sent = Sent { path: saved.map(|saved| saved.path.clone()), name, thumbnail, ..Sent::default() };
        match self.send(kind, request, bytes, sent, self.settings.after_upload).await {
            Ok(url) => Ok(url),
            Err(Ended::Stopped) => Err(Halt),
            Err(Ended::Failed(error)) => Err(self.fail(error)),
        }
    }

    /// A file a recording session already saved, so nothing is written here. Its save is reported as
    /// `Saved`. Then the steps that work on a saved file, as for a saved image: copy the file (or its path, or its
    /// folder), reveal it. Then the upload (when the upload flag is set), the after-upload steps and the toast.
    pub(super) async fn saved_file(&mut self, path: PathBuf) -> Step {
        self.checkpoint()?;
        self.cx.counter.fetch_add(1, Ordering::SeqCst);
        let saved = SavedFile { path };
        self.emit(TaskEvent::Saved { id: self.id, path: saved.path.clone() });
        let flags = self.settings.after_capture;
        // One clipboard copy, as for an image: the file wins over its path, the path over its folder.
        if flags.contains(AfterCaptureTasks::COPY_FILE_TO_CLIPBOARD) {
            self.copy_file(Some(&saved)).await?;
        } else if flags.contains(AfterCaptureTasks::COPY_FILE_PATH_TO_CLIPBOARD) {
            self.copy_path(Some(&saved)).await?;
        } else if flags.contains(AfterCaptureTasks::COPY_FOLDER_PATH_TO_CLIPBOARD) {
            self.copy_folder(Some(&saved)).await?;
        }
        if flags.contains(AfterCaptureTasks::SHOW_IN_EXPLORER) {
            self.reveal(Some(&saved))?;
        }
        let url = if flags.contains(AfterCaptureTasks::UPLOAD_IMAGE_TO_HOST) { self.upload_saved(&saved).await? } else { None };
        if self.settings.general.show_toast {
            self.toast(Some(&saved), None, url)?;
        }
        self.complete();
        Ok(())
    }

    /// The upload of a saved file: the before-upload window first (Cancel skips the upload), then the file itself.
    async fn upload_saved(&mut self, saved: &SavedFile) -> Step<Option<String>> {
        let name = upload::file_name_of(&saved.path);
        let kind = upload::kind_for_name(&name);
        if self.settings.after_capture.contains(AfterCaptureTasks::SHOW_BEFORE_UPLOAD_WINDOW)
            && !self.choose_destination(kind.clone(), &name).await?
        {
            return Ok(None);
        }
        let size = match files::size(self.cx.ports.spawner.as_ref(), saved.path.clone()).await {
            Ok(size) => size,
            Err(error) => return Err(self.fail(AppError::Io(error.to_string()))),
        };
        let request = UploadRequest {
            file_name: name.clone(),
            kind: kind.clone(),
            data: UploadData::Path(saved.path.clone()),
            mime: upload::mime_for_name(&name).to_owned(),
        };
        let sent = Sent { path: Some(saved.path.clone()), name, ..Sent::default() };
        match self.send(kind, request, size, sent, self.settings.after_upload).await {
            Ok(url) => Ok(url),
            Err(Ended::Stopped) => Err(Halt),
            Err(Ended::Failed(error)) => Err(self.fail(error)),
        }
    }

    /// The before-upload window. Returns false when the upload should be skipped: the user cancelled, or the
    /// window could not be shown. The destination the user picked is used for this job's upload of `kind`.
    async fn choose_destination(&mut self, kind: UploadKind, name: &str) -> Step<bool> {
        self.checkpoint()?;
        let destinations = self.cx.ports.uploaders.destination_list();
        let current = self.destination_slot(&kind).clone();
        let answer = self.cx.ports.prompt.before_upload(name, &destinations, &current).await;
        match answer {
            Ok(id) => {
                *self.destination_slot(&kind) = id;
                Ok(true)
            }
            Err(PortError::Cancelled) => Ok(false),
            Err(error) => {
                self.step_failed("before_upload", error);
                Ok(false)
            }
        }
    }

    /// The destination id this job uses for uploads of `kind`.
    fn destination_slot(&mut self, kind: &UploadKind) -> &mut String {
        let upload = &mut self.settings.upload;
        match kind {
            UploadKind::Image => &mut upload.image_destination,
            UploadKind::Text => &mut upload.text_destination,
            UploadKind::File => &mut upload.file_destination,
        }
    }

    /// Runs one clipboard call on the blocking pool, so the OS clipboard never stalls the async worker. A panic in
    /// the call resumes here, so the job reports it as it would from the call itself (ADR 020).
    async fn clipboard<T: Send + 'static>(
        &self,
        call: impl FnOnce(&dyn Clipboard) -> Result<T, PortError> + Send + 'static,
    ) -> Result<T, PortError> {
        let clipboard = Arc::clone(&self.cx.ports.clipboard);
        let (tx, rx) = oneshot::channel();
        self.cx
            .ports
            .spawner
            .run_blocking(Box::new(move || {
                let outcome = std::panic::catch_unwind(AssertUnwindSafe(move || call(&*clipboard)));
                let _ = tx.send(outcome);
            }))
            .await;
        match rx.await {
            Ok(Ok(result)) => result,
            Ok(Err(payload)) => std::panic::resume_unwind(payload),
            Err(oneshot::Canceled) => Err(PortError::Cancelled),
        }
    }

    async fn copy_image(&self, image: &Bitmap) -> Step {
        self.checkpoint()?;
        let image = image.clone();
        match self.clipboard(move |clipboard| clipboard.set_image(&image)).await {
            Ok(()) => self.emit(TaskEvent::Copied { id: self.id }),
            Err(error) => self.emit(TaskEvent::CopyFailed { id: self.id, reason: error.to_string() }),
        }
        Ok(())
    }

    /// Writes the thumbnail beside the saved image. Returns its path when it was written.
    async fn save_thumbnail(&self, saved: Option<&SavedFile>, made: Option<Result<Encoded, AppError>>) -> Step<Option<PathBuf>> {
        let Some(saved) = saved else {
            self.step_failed("thumbnail", self.no_file_reason());
            return Ok(None);
        };
        let Some(made) = made else { return Ok(None) };
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

    async fn copy_file(&self, saved: Option<&SavedFile>) -> Step {
        let Some(saved) = saved else {
            self.step_failed("copy_file", self.no_file_reason());
            return Ok(());
        };
        self.checkpoint()?;
        let path = saved.path.clone();
        match self.clipboard(move |clipboard| clipboard.set_files(std::slice::from_ref(&path))).await {
            Ok(()) => self.emit(TaskEvent::FileCopied { id: self.id }),
            Err(error) => self.step_failed("copy_file", error),
        }
        Ok(())
    }

    async fn copy_path(&self, saved: Option<&SavedFile>) -> Step {
        let Some(saved) = saved else {
            self.step_failed("copy_path", self.no_file_reason());
            return Ok(());
        };
        self.checkpoint()?;
        let text = saved.path.display().to_string();
        match self.clipboard(move |clipboard| clipboard.set_text(&text)).await {
            Ok(()) => self.emit(TaskEvent::PathCopied { id: self.id, path: saved.path.clone() }),
            Err(error) => self.step_failed("copy_path", error),
        }
        Ok(())
    }

    /// `COPY_FOLDER_PATH_TO_CLIPBOARD`: the saved file's folder.
    async fn copy_folder(&self, saved: Option<&SavedFile>) -> Step {
        let Some(dir) = saved.and_then(|s| s.path.parent()) else {
            self.step_failed("copy_folder_path", self.no_file_reason());
            return Ok(());
        };
        self.checkpoint()?;
        let text = dir.display().to_string();
        match self.clipboard(move |clipboard| clipboard.set_text(&text)).await {
            Ok(()) => self.emit(TaskEvent::PathCopied { id: self.id, path: dir.to_path_buf() }),
            Err(error) => self.step_failed("copy_folder_path", error),
        }
        Ok(())
    }

    /// `SEND_IMAGE_TO_PRINTER`: the saved file goes to the default printer. With no saved file, the
    /// image is written to a temporary file that is printed and removed `PRINT_FILE_LIFETIME` later.
    async fn print(&self, saved: Option<&SavedFile>, encoded: Option<Encoded>) -> Step {
        self.checkpoint()?;
        if let Some(saved) = saved {
            match self.cx.ports.shell.print(&saved.path) {
                Ok(()) => self.emit(TaskEvent::Printed { id: self.id, path: saved.path.clone() }),
                Err(error) => self.step_failed("print", error),
            }
            return Ok(());
        }
        let Some(encoded) = encoded else {
            self.step_failed("print", "the image could not be encoded for printing");
            return Ok(());
        };
        let spawner = self.cx.ports.spawner.as_ref();
        // Task ids restart with each run and each instance, so the name also carries the process and a
        // random part: two instances, or a restart, never share a print file.
        let unique = format!("{}-{}-{:016x}", std::process::id(), self.id.0, self.cx.ports.rng.next_u64());
        let temp = std::env::temp_dir().join(format!("laqta-print-{unique}.{}", encoded.format.extension()));
        if let Err(error) = files::save_atomic(spawner, temp.clone(), encoded.bytes).await {
            self.step_failed("print", error);
            return Ok(());
        }
        match self.cx.ports.shell.print(&temp) {
            Ok(()) => self.emit(TaskEvent::Printed { id: self.id, path: temp.clone() }),
            Err(error) => self.step_failed("print", error),
        }
        // Windows' print verb returns before the print handler opens the file, so the file stays a
        // while. If the app quits first, it is left in the temporary folder.
        let ports = Arc::clone(&self.cx.ports);
        self.cx.ports.spawner.spawn(Box::pin(async move {
            ports.clock.sleep(PRINT_FILE_LIFETIME).await;
            let _ = files::remove(ports.spawner.as_ref(), temp).await;
        }));
        Ok(())
    }

    /// `PIN_TO_SCREEN`: the capture in an always-on-top window.
    fn pin(&self, image: &Bitmap) -> Step {
        self.checkpoint()?;
        match self.cx.ports.tools.pin(image.clone()) {
            Ok(()) => self.emit(TaskEvent::Pinned { id: self.id }),
            Err(error) => self.step_failed("pin", error),
        }
        Ok(())
    }

    /// Shows a text result and copies it (QR codes, OCR).
    fn found(&self, step: &'static str, title: &str, text: String) {
        if !text.is_empty() {
            let _ = self.cx.ports.clipboard.set_text(&text);
            if let Err(error) = self.cx.ports.tools.show_text(title, &text) {
                self.step_failed(step, error);
            }
        }
        self.emit(TaskEvent::TextFound { id: self.id, step, text });
    }

    /// `SCAN_QR_CODE`: every QR code or barcode in the capture, on the compute pool.
    async fn scan_qr(&self, image: &Bitmap) -> Step {
        self.checkpoint()?;
        let image = image.clone();
        match compute(self.cx.ports.spawner.as_ref(), move || Ok(laqta_imaging::qr::qr_decode(&image))).await {
            Ok(codes) => self.found("scan_qr", "QR code (copied)", codes.join("\n")),
            Err(error) => self.step_failed("scan_qr", error),
        }
        Ok(())
    }

    /// `REMOVE_BACKGROUND`: the `BackgroundRemover` port. A failure is a step failure and the
    /// image is kept.
    async fn remove_background(&self, image: Bitmap) -> Bitmap {
        match self.cx.ports.background.remove(image.clone()).await {
            Ok(cut) => {
                self.emit(TaskEvent::BackgroundRemoved { id: self.id });
                cut
            }
            Err(error) => {
                self.step_failed("remove_background", error);
                image
            }
        }
    }

    /// `ANALYZE_IMAGE`: the configured analyzer's answer to the configured prompt.
    async fn analyze(&self, image: &Bitmap) -> Step {
        self.checkpoint()?;
        let prompt = self.cx.settings.get().cloud.analyzer.prompt.clone();
        match self.cx.ports.analyzer.analyze(image.clone(), prompt).await {
            Ok(answer) => self.found("analyze", "Image analysis (copied)", answer),
            Err(error) => self.step_failed("analyze", error),
        }
        Ok(())
    }

    /// `DO_OCR`: the capture's text through the OCR port (ADR 023).
    async fn ocr(&self, image: &Bitmap) -> Step {
        self.checkpoint()?;
        match self.cx.ports.ocr.recognize(image.clone(), String::new()).await {
            Ok(result) => self.found("ocr", "Recognised text (copied)", result.text()),
            Err(error) => self.step_failed("ocr", error),
        }
        Ok(())
    }

    fn reveal(&self, saved: Option<&SavedFile>) -> Step {
        let Some(saved) = saved else {
            self.step_failed("reveal", self.no_file_reason());
            return Ok(());
        };
        self.checkpoint()?;
        match self.cx.ports.shell.reveal(&saved.path) {
            Ok(()) => self.emit(TaskEvent::Revealed { id: self.id, path: saved.path.clone() }),
            Err(error) => self.step_failed("reveal", error),
        }
        Ok(())
    }

    /// The toast for a capture, or for a saved file (`preview` is None: no image). `url` is the upload's URL,
    /// shortened when shortening worked.
    fn toast(&self, saved: Option<&SavedFile>, preview: Option<Bitmap>, url: Option<String>) -> Step {
        self.checkpoint()?;
        let (title, body, path) = match saved {
            Some(saved) if preview.is_none() && is_recording(&saved.path) => {
                ("Recording saved", where_saved(&saved.path), Some(saved.path.clone()))
            }
            Some(saved) => ("Screenshot saved", where_saved(&saved.path), Some(saved.path.clone())),
            None => match &self.save_error {
                Some(error) => ("Screenshot not saved", error.clone(), None),
                None => ("Screenshot copied", String::new(), None),
            },
        };
        let toast = Toast { title: title.to_owned(), body, image: preview, path, url, options: self.settings.general.toast.clone() };
        match self.cx.ports.notifier.toast(toast) {
            Ok(()) => self.emit(TaskEvent::Notified { id: self.id }),
            Err(error) => self.step_failed("toast", error),
        }
        Ok(())
    }

    /// The `DELETE_FILE` step removes the file the job wrote, however the job ended. A
    /// stop does not skip it, so there is no checkpoint. A file that is already gone is not an error.
    pub(super) async fn finally(&mut self) {
        if !self.settings.after_capture.contains(AfterCaptureTasks::DELETE_FILE) {
            return;
        }
        let Some(path) = self.file.take() else { return };
        let spawner = self.cx.ports.spawner.as_ref();
        if !files::exists(spawner, path.clone()).await {
            return;
        }
        match files::remove(spawner, path.clone()).await {
            Ok(()) => self.emit(TaskEvent::Deleted { id: self.id, path }),
            Err(error) => self.step_failed("delete", error),
        }
    }

    /// Stops the job when a stop was requested. Emits `Stopping`, then `Stopped`. A saved file stays, unless
    /// `DELETE_FILE` is set: `finally` removes it.
    pub(super) fn checkpoint(&self) -> Step {
        if !self.stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        self.cx.reporter.status(self.id, TaskStatus::Stopping);
        self.cx.reporter.status(self.id, TaskStatus::Stopped);
        Err(Halt)
    }

    /// Stops the job at a prompt the user cancelled. Only `Stopped` is reported, and no file is kept back.
    fn cancelled(&self) -> Halt {
        self.cx.reporter.status(self.id, TaskStatus::Stopped);
        Halt
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
        if general.play_sound_after_task && !self.upload_sound.load(std::sync::atomic::Ordering::Relaxed) {
            let _ = self.cx.ports.notifier.sound(laqta_ports::Sound::TaskCompleted, general.custom_task_sound.as_deref());
        }
        self.cx.reporter.status(self.id, TaskStatus::Completed);
    }

    /// The task-completed sound after a successful upload, when `play_sound_after_upload` is set
    /// (ShareX's after-upload sound). Not played on failure.
    pub(super) fn upload_done(&self) {
        let general = &self.settings.general;
        if general.play_sound_after_upload {
            let _ = self.cx.ports.notifier.sound(laqta_ports::Sound::TaskCompleted, general.custom_task_sound.as_deref());
            self.upload_sound.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    pub(super) fn step_failed(&self, step: &'static str, error: impl Display) {
        self.emit(TaskEvent::StepFailed { id: self.id, step, reason: error.to_string() });
    }

    pub(super) fn emit(&self, event: TaskEvent) {
        self.cx.reporter.emit(event);
    }
}

/// The quick menu's flags for a choice. `None` is Cancel, or an index the menu does not have.
fn quick_menu_flags(choice: usize) -> Option<AfterCaptureTasks> {
    use AfterCaptureTasks as A;
    Some(match choice {
        0 => A::SAVE_IMAGE_TO_FILE | A::COPY_IMAGE_TO_CLIPBOARD,
        1 => A::SAVE_IMAGE_TO_FILE,
        2 => A::COPY_IMAGE_TO_CLIPBOARD,
        3 => A::UPLOAD_IMAGE_TO_HOST,
        4 => A::SAVE_IMAGE_TO_FILE | A::UPLOAD_IMAGE_TO_HOST,
        5 => A::ANNOTATE_IMAGE | A::SAVE_IMAGE_TO_FILE | A::COPY_IMAGE_TO_CLIPBOARD,
        _ => return None,
    })
}

/// The name that picks a random preset each time.
pub const RANDOM_PRESET: &str = "*";

/// The preset named `name`, the first preset when `name` is empty, or one picked by `roll` for `*`.
fn pick_preset<'a>(presets: &'a [Preset], name: &str, roll: u64) -> Option<&'a Preset> {
    match name {
        "" => presets.first(),
        RANDOM_PRESET if !presets.is_empty() => presets.get((roll % presets.len() as u64) as usize),
        _ => presets.iter().find(|preset| preset.name == name),
    }
}

/// Whether a step works on the composed image: the after-capture window, annotation, effects or the beautifier.
fn needs_pixels(flags: AfterCaptureTasks) -> bool {
    flags.intersects(
        AfterCaptureTasks::SHOW_AFTER_CAPTURE_WINDOW
            | AfterCaptureTasks::ANNOTATE_IMAGE
            | AfterCaptureTasks::ADD_IMAGE_EFFECTS
            | AfterCaptureTasks::BEAUTIFY_IMAGE
            | AfterCaptureTasks::REMOVE_BACKGROUND,
    )
}

/// Whether any step reads a file, or uploads the image, so the image is encoded once for all of them.
fn needs_file(flags: AfterCaptureTasks) -> bool {
    flags.intersects(
        AfterCaptureTasks::SAVE_IMAGE_TO_FILE
            | AfterCaptureTasks::SAVE_IMAGE_TO_FILE_WITH_DIALOG
            | AfterCaptureTasks::SAVE_THUMBNAIL_IMAGE_TO_FILE
            | AfterCaptureTasks::COPY_FILE_TO_CLIPBOARD
            | AfterCaptureTasks::COPY_FILE_PATH_TO_CLIPBOARD
            | AfterCaptureTasks::SHOW_IN_EXPLORER
            | AfterCaptureTasks::UPLOAD_IMAGE_TO_HOST
            | AfterCaptureTasks::SEND_IMAGE_TO_PRINTER
            | AfterCaptureTasks::DELETE_FILE,
    )
}

/// A screen recording's output, by extension (FFmpeg's containers and GIF).
fn is_recording(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| ["mp4", "webm", "mkv", "mov", "avi", "gif"].iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// The toast's second line: the file name and the folder it went to, not a full path that
/// truncates before the part people read. "Laqta_2026-11-14_08-03.png in 2026-11".
fn where_saved(path: &std::path::Path) -> String {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match path.parent().and_then(|p| p.file_name()) {
        Some(dir) => format!("{name} in {}", dir.to_string_lossy()),
        None => name,
    }
}

#[cfg(test)]
mod preset_tests {
    use super::*;

    #[test]
    fn random_preset_uses_the_roll() {
        let presets: Vec<Preset> = ["a", "b", "c"].iter().map(|n| Preset { name: (*n).into(), effects: vec![] }).collect();
        assert_eq!(pick_preset(&presets, "", 7).map(|p| p.name.as_str()), Some("a"));
        assert_eq!(pick_preset(&presets, "b", 7).map(|p| p.name.as_str()), Some("b"));
        assert_eq!(pick_preset(&presets, RANDOM_PRESET, 4).map(|p| p.name.as_str()), Some("b"));
        assert_eq!(pick_preset(&presets, RANDOM_PRESET, 5).map(|p| p.name.as_str()), Some("c"));
        assert!(pick_preset(&[], RANDOM_PRESET, 1).is_none());
    }
}
