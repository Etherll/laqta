//! The upload phase and the after-upload steps (M2 §4, ADR 011). Every upload goes through `Job::send`: the
//! destination, the large-file warning, the upload with bounded retries (risk R7), and then the after-upload
//! steps, which run only when the host returned a URL. The image of a capture job and the Upload jobs
//! (text, file, URL to download, URL to shorten) share these steps.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use laqta_domain::naming::{self, UploadInfo};
use laqta_domain::workflow::{AfterUploadTasks, TaskSettings};
use laqta_ports::{AppError, ProgressCallback, TaskEvent, TaskId, TaskStatus, UploadData, UploadKind, UploadRequest, UploadResult};

use super::run::{Halt, Job, Step};
use super::{CaptureInfo, Context, UploadSource, save};
use crate::files;

/// Progress reports for a total of unknown size are spaced this many bytes apart.
const UNKNOWN_TOTAL_STEP: u64 = 64 * 1024;

/// How an upload phase ended without a result.
#[derive(Debug)]
pub(super) enum Ended {
    /// A stop was requested, and it is already reported.
    Stopped,
    /// The upload failed. The job reports it, after any cleanup.
    Failed(AppError),
}

impl From<Halt> for Ended {
    fn from(_: Halt) -> Ended {
        Ended::Stopped
    }
}

/// The file an upload sent, as the upload-info tokens see it.
#[derive(Clone, Debug, Default)]
pub(super) struct Sent {
    /// The file on disk, when the upload has one.
    pub(super) path: Option<PathBuf>,
    pub(super) name: String,
    pub(super) thumbnail: Option<PathBuf>,
    /// The job only shortens a URL, so the shortened URL is what `$url` means in the clipboard text.
    pub(super) shorten_only: bool,
}

/// Runs an Upload job. Its statuses follow a capture's: `Preparing`, `Working`, then the last step.
pub(super) async fn run(cx: Context, id: TaskId, source: UploadSource, settings: TaskSettings, stop: Arc<AtomicBool>) {
    cx.reporter.status(id, TaskStatus::Preparing);
    cx.reporter.status(id, TaskStatus::Working);
    let job = Job { cx, id, settings, info: CaptureInfo::default(), stop };
    // The outcome is already reported, so there is nothing to do with it here.
    let _ = job.run_upload(source).await;
}

impl Job {
    /// The body of an Upload job.
    async fn run_upload(&self, source: UploadSource) -> Step {
        self.checkpoint()?;
        match source {
            UploadSource::Text(text) => {
                let name = self.generated_name("txt", None);
                let bytes = byte_len(text.len());
                let request = UploadRequest {
                    file_name: name.clone(),
                    kind: UploadKind::Text,
                    data: UploadData::Bytes(text.into_bytes()),
                    mime: mime_for_name(&name).to_owned(),
                };
                self.send_job(UploadKind::Text, request, bytes, Sent { name, ..Sent::default() }).await
            }
            UploadSource::File(path) => {
                let size = match files::size(self.cx.ports.spawner.as_ref(), path.clone()).await {
                    Ok(size) => size,
                    Err(error) => return Err(self.fail(AppError::Io(error.to_string()))),
                };
                let name = file_name_of(&path);
                let kind = kind_for_name(&name);
                let request = UploadRequest {
                    file_name: name.clone(),
                    kind: kind.clone(),
                    data: UploadData::Path(path.clone()),
                    mime: mime_for_name(&name).to_owned(),
                };
                self.send_job(kind, request, size, Sent { path: Some(path), name, ..Sent::default() }).await
            }
            UploadSource::Url(url) => {
                let (bytes, name, mime) = match self.cx.ports.uploaders.download(&url).await {
                    Ok(downloaded) => downloaded,
                    Err(error) => return Err(self.fail(AppError::from(error))),
                };
                let kind = kind_for_name(&name);
                let mime = if mime.is_empty() { mime_for_name(&name).to_owned() } else { mime };
                let size = byte_len(bytes.len());
                let request = UploadRequest { file_name: name.clone(), kind: kind.clone(), data: UploadData::Bytes(bytes), mime };
                self.send_job(kind, request, size, Sent { name, ..Sent::default() }).await
            }
            UploadSource::ShortenUrl(url) => {
                // Nothing is uploaded: the given URL is the one the after-upload steps work on.
                let result = UploadResult { url: Some(url), ..UploadResult::default() };
                let flags = self.settings.after_upload | AfterUploadTasks::USE_URL_SHORTENER;
                let sent = Sent { shorten_only: true, ..Sent::default() };
                self.after_upload(&result, &sent, flags).await?;
                self.complete();
                Ok(())
            }
        }
    }

    /// Uploads for an Upload job, then ends the job: `Completed`, or the failure or stop already reported.
    async fn send_job(&self, kind: UploadKind, request: UploadRequest, size: u64, sent: Sent) -> Step {
        match self.send(kind, request, size, sent, self.settings.after_upload).await {
            Ok(_) => {
                self.complete();
                Ok(())
            }
            Err(Ended::Stopped) => Err(Halt),
            Err(Ended::Failed(error)) => Err(self.fail(error)),
        }
    }

    /// Uploads `request`, and runs the after-upload steps (`flags`) when the host returned a URL. Returns the URL
    /// the steps ended with: the shortened one when shortening worked. `None` means no URL came back, so the
    /// after-upload steps were skipped.
    pub(super) async fn send(
        &self,
        kind: UploadKind,
        request: UploadRequest,
        size: u64,
        sent: Sent,
        flags: AfterUploadTasks,
    ) -> Result<Option<String>, Ended> {
        let result = self.upload(kind, request, size).await?;
        let Some(url) = result.url.clone() else { return Ok(None) };
        self.emit(TaskEvent::Uploaded {
            id: self.id,
            url,
            thumbnail_url: result.thumbnail_url.clone(),
            deletion_url: result.deletion_url.clone(),
        });
        Ok(Some(self.after_upload(&result, &sent, flags).await?))
    }

    /// The upload itself: the destination, the large-file warning, then the attempts. A network error, an HTTP
    /// 5xx or a 429 is retried `settings.upload.retries` times, waiting 1 s and then 3 s (risk R7). Any other
    /// error fails at once.
    async fn upload(&self, kind: UploadKind, request: UploadRequest, size: u64) -> Result<UploadResult, Ended> {
        let settings = &self.settings.upload;
        let destination_id = match kind {
            UploadKind::Image => &settings.image_destination,
            UploadKind::Text => &settings.text_destination,
            UploadKind::File => &settings.file_destination,
        };
        let Some(destination) = self.cx.ports.uploaders.destination(destination_id) else {
            return Err(Ended::Failed(AppError::Unsupported(format!("no destination {destination_id}"))));
        };
        self.checkpoint()?;
        if settings.large_file_warning_bytes > 0 && size > settings.large_file_warning_bytes {
            // The title and text are the file and its size. The dialog's wording is the UI's to set.
            let confirmed = self
                .cx
                .ports
                .prompt
                .confirm(&request.file_name, &format!("{size} bytes"))
                .await
                .map_err(|error| Ended::Failed(AppError::from(error)))?;
            if !confirmed {
                return Err(Ended::Failed(AppError::Cancelled));
            }
        }
        self.emit(TaskEvent::UploadStarted { id: self.id, destination: destination_id.clone() });
        let mut retries = 0;
        loop {
            match destination.upload(request.clone(), self.progress_callback()).await {
                Ok(result) => return Ok(result),
                Err(error) if error.is_retryable() && retries < settings.retries => {
                    retries += 1;
                    self.emit(TaskEvent::UploadRetry { id: self.id, attempt: retries, reason: error.to_string() });
                    self.cx.ports.clock.sleep(retry_wait(retries)).await;
                    self.checkpoint()?;
                }
                Err(error) => return Err(Ended::Failed(AppError::from(error))),
            }
        }
    }

    /// The progress callback for one attempt. Reports go through `progress_due`, so a busy adapter does not
    /// flood the event channel.
    fn progress_callback(&self) -> ProgressCallback {
        let reporter = self.cx.reporter.clone();
        let id = self.id;
        let last = Mutex::new(None::<u64>);
        Arc::new(move |sent: u64, total: Option<u64>| {
            let mut last = last.lock().unwrap_or_else(PoisonError::into_inner);
            if progress_due(*last, sent, total) {
                *last = Some(sent);
                reporter.emit(TaskEvent::Progress { id, sent, total });
            }
        })
    }

    /// The after-upload steps, in the order of the flags (M2 §4). Each step runs only when its flag is set. A
    /// failed step is reported as `StepFailed`, and the next step still runs. A stop is checked before each step.
    /// Returns the URL the steps ended with.
    async fn after_upload(&self, result: &UploadResult, sent: &Sent, flags: AfterUploadTasks) -> Step<String> {
        let id = self.id;
        let direct = result.url.clone().unwrap_or_default();
        let mut url = direct.clone();
        let mut short: Option<String> = None;

        if flags.contains(AfterUploadTasks::USE_URL_SHORTENER) {
            self.checkpoint()?;
            match self.shorten(&direct).await {
                Ok(shortened) => {
                    self.emit(TaskEvent::Shortened { id, url: shortened.clone() });
                    url = shortened.clone();
                    short = Some(shortened);
                }
                Err(reason) => self.step_failed("shorten", reason),
            }
        }
        if flags.contains(AfterUploadTasks::SHARE_URL) {
            self.checkpoint()?;
            match self.share(&url) {
                Ok(page) => match self.cx.ports.shell.open_url(&page) {
                    Ok(()) => self.emit(TaskEvent::Shared { id, url: page }),
                    Err(error) => self.step_failed("share", error),
                },
                Err(reason) => self.step_failed("share", reason),
            }
        }
        if flags.contains(AfterUploadTasks::COPY_URL_TO_CLIPBOARD) {
            self.checkpoint()?;
            // A shortening job copies the URL it shortened to, so `$url` is the short one there.
            let copied = if sent.shorten_only { url.as_str() } else { direct.as_str() };
            let info = UploadInfo {
                url: copied,
                short_url: short.as_deref().unwrap_or_default(),
                thumbnail_url: result.thumbnail_url.as_deref().unwrap_or_default(),
                deletion_url: result.deletion_url.as_deref().unwrap_or_default(),
                result: &result.response,
                file_path: sent.path.as_deref(),
                file_name: &sent.name,
                thumbnail_path: sent.thumbnail.as_deref(),
                upload_time: self.cx.ports.clock.now(),
            };
            let text = naming::upload_info(&self.settings.upload.clipboard_format, &info);
            match self.cx.ports.clipboard.set_text(&text) {
                Ok(()) => self.emit(TaskEvent::UrlCopied { id, text }),
                Err(error) => self.step_failed("copy_url", error),
            }
        }
        if flags.contains(AfterUploadTasks::OPEN_URL) {
            self.checkpoint()?;
            match self.cx.ports.shell.open_url(&url) {
                Ok(()) => self.emit(TaskEvent::UrlOpened { id, url: url.clone() }),
                Err(error) => self.step_failed("open_url", error),
            }
        }
        if flags.contains(AfterUploadTasks::SHOW_QR_CODE) {
            self.checkpoint()?;
            self.emit(TaskEvent::ShowUrl { id, url: url.clone(), qr: true });
        }
        if flags.contains(AfterUploadTasks::SHOW_AFTER_UPLOAD_WINDOW) {
            self.checkpoint()?;
            self.emit(TaskEvent::ShowUrl { id, url: url.clone(), qr: false });
        }
        Ok(url)
    }

    /// Shortens `url` with the configured shortener. The error is the reason, for `StepFailed`.
    async fn shorten(&self, url: &str) -> Result<String, String> {
        let id = &self.settings.upload.url_shortener;
        let Some(shortener) = self.cx.ports.uploaders.shortener(id) else {
            return Err(format!("no shortener {id}"));
        };
        shortener.shorten(url).await.map_err(|error| error.to_string())
    }

    /// The page to open for `url` with the configured sharing service.
    fn share(&self, url: &str) -> Result<String, String> {
        let id = &self.settings.upload.url_sharer;
        let Some(sharer) = self.cx.ports.uploaders.sharer(id) else {
            return Err(format!("no sharer {id}"));
        };
        sharer.share_url(url).map_err(|error| error.to_string())
    }

    /// A file name for an upload that has no file on disk: the naming pattern with `ext`.
    pub(super) fn generated_name(&self, ext: &str, size: Option<(u32, u32)>) -> String {
        save::generated_name(&self.cx, &self.settings, &self.info, size, ext)
    }
}

/// Retry `retry` (1-based) waits 1 s before the first retry and 3 s before every later one.
fn retry_wait(retry: u32) -> Duration {
    if retry <= 1 { Duration::from_secs(1) } else { Duration::from_secs(3) }
}

/// Whether a progress report is due. The first report always is. After that one is due when the upload moved
/// by 1 % of its total (64 KiB when the total is unknown), or when it reached the total. A repeated value is
/// never reported again.
fn progress_due(last: Option<u64>, sent: u64, total: Option<u64>) -> bool {
    match last {
        None => true,
        Some(last) if last == sent => false,
        Some(last) => {
            let step = total.map_or(UNKNOWN_TOTAL_STEP, |total| (total / 100).max(1));
            sent.saturating_sub(last) >= step || total.is_some_and(|total| sent >= total)
        }
    }
}

pub(super) fn byte_len(len: usize) -> u64 {
    u64::try_from(len).unwrap_or(u64::MAX)
}

pub(super) fn file_name_of(path: &Path) -> String {
    path.file_name().map_or_else(|| "file".to_owned(), |name| name.to_string_lossy().into_owned())
}

const IMAGE_EXTENSIONS: [&str; 8] = ["png", "jpg", "jpeg", "gif", "bmp", "webp", "tif", "tiff"];
const TEXT_EXTENSIONS: [&str; 11] = ["txt", "text", "log", "md", "csv", "json", "xml", "html", "htm", "css", "js"];

fn extension_of(name: &str) -> Option<String> {
    Path::new(name).extension().and_then(|ext| ext.to_str()).map(str::to_ascii_lowercase)
}

/// The destination kind for a file, by its extension: images go to the image host, text to the text host,
/// and anything else to the file host.
pub(super) fn kind_for_name(name: &str) -> UploadKind {
    match extension_of(name).as_deref() {
        Some(ext) if IMAGE_EXTENSIONS.contains(&ext) => UploadKind::Image,
        Some(ext) if TEXT_EXTENSIONS.contains(&ext) => UploadKind::Text,
        _ => UploadKind::File,
    }
}

/// The mime type of a file, from its extension. An unknown extension is `application/octet-stream`.
pub(super) fn mime_for_name(name: &str) -> &'static str {
    match extension_of(name).as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("bmp") => "image/bmp",
        Some("webp") => "image/webp",
        Some("tif" | "tiff") => "image/tiff",
        Some("txt" | "text" | "log") => "text/plain",
        Some("md") => "text/markdown",
        Some("csv") => "text/csv",
        Some("html" | "htm") => "text/html",
        Some("css") => "text/css",
        Some("js") => "text/javascript",
        Some("json") => "application/json",
        Some("xml") => "application/xml",
        Some("pdf") => "application/pdf",
        Some("zip") => "application/zip",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_reports_the_first_value_then_each_percent() {
        assert!(progress_due(None, 0, Some(1000)));
        assert!(!progress_due(Some(10), 10, Some(1000)), "a repeated value is not reported");
        assert!(!progress_due(Some(10), 15, Some(1000)), "less than 1 % moved");
        assert!(progress_due(Some(10), 20, Some(1000)), "1 % moved");
        assert!(progress_due(Some(995), 1000, Some(1000)), "the end is always reported");
    }

    #[test]
    fn progress_with_unknown_total_steps_by_64_kib() {
        assert!(!progress_due(Some(0), 1000, None));
        assert!(progress_due(Some(0), UNKNOWN_TOTAL_STEP, None));
    }

    #[test]
    fn retries_wait_one_then_three_seconds() {
        assert_eq!(retry_wait(1), Duration::from_secs(1));
        assert_eq!(retry_wait(2), Duration::from_secs(3));
        assert_eq!(retry_wait(5), Duration::from_secs(3));
    }

    #[test]
    fn files_go_to_the_destination_for_their_kind() {
        assert_eq!(kind_for_name("shot.PNG"), UploadKind::Image);
        assert_eq!(kind_for_name("notes.md"), UploadKind::Text);
        assert_eq!(kind_for_name("archive.zip"), UploadKind::File);
        assert_eq!(kind_for_name("no-extension"), UploadKind::File);
    }

    #[test]
    fn mime_comes_from_the_extension() {
        assert_eq!(mime_for_name("a.jpeg"), "image/jpeg");
        assert_eq!(mime_for_name("a.unknown"), "application/octet-stream");
        assert_eq!(mime_for_name("a"), "application/octet-stream");
    }
}
