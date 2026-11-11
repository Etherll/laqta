//! The `.sxcu` Destination and Shortener: one custom uploader run through the engine and
//! `laqta_http::send_with_progress`. Prompt answers are remembered for the life of the destination,
//! so an `{inputbox}` asks once instead of on every evaluation.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use laqta_http::HttpError;
use laqta_ports::{
    BoxFuture, Destination, PortError, ProgressCallback, Prompt, Rng, Sharer, Shortener, UploadData, UploadError, UploadKind,
    UploadRequest, UploadResult,
};

use crate::sxcu::{CustomUploader, Env, Input, build_request, parse_response};

/// A `.sxcu` file used as a Destination.
pub struct SxcuDestination {
    id: String,
    uploader: CustomUploader,
    prompt: Arc<dyn Prompt>,
    rng: Arc<dyn Rng>,
}

impl SxcuDestination {
    pub fn new(id: impl Into<String>, uploader: CustomUploader, prompt: Arc<dyn Prompt>, rng: Arc<dyn Rng>) -> Self {
        SxcuDestination { id: id.into(), uploader, prompt: Arc::new(Remembered::new(prompt)), rng }
    }

    /// Builds the request, sends it and reads the response.
    async fn exchange(&self, input: &Input<'_>, progress: ProgressCallback) -> Result<UploadResult, UploadError> {
        let env = Env { prompt: self.prompt.as_ref(), rng: self.rng.as_ref() };
        let request = build_request(&self.uploader, input, &env).await?;
        let response = laqta_http::send_with_progress(request, progress).await.map_err(http_error)?;
        parse_response(&self.uploader, &response, input, &env).await
    }
}

impl Destination for SxcuDestination {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        self.uploader.name.as_deref().unwrap_or(self.id.as_str())
    }

    fn upload(&self, request: UploadRequest, progress: ProgressCallback) -> BoxFuture<'_, Result<UploadResult, UploadError>> {
        Box::pin(async move {
            let data = match request.data {
                UploadData::Bytes(bytes) => bytes,
                UploadData::Path(path) => {
                    std::fs::read(&path).map_err(|error| UploadError::BadResponse(format!("cannot read {}: {error}", path.display())))?
                }
            };
            // A text upload is also the `{input}` of the file; images and files have none.
            let text = (request.kind == UploadKind::Text).then(|| String::from_utf8_lossy(&data).into_owned());
            let input = Input { file_name: &request.file_name, data: &data, mime: &request.mime, text: text.as_deref() };
            self.exchange(&input, progress).await
        })
    }
}

impl fmt::Debug for SxcuDestination {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SxcuDestination").field("id", &self.id).finish_non_exhaustive()
    }
}

/// A `.sxcu` file used as a URL shortener. The URL to shorten is the file's `{input}`.
pub struct SxcuShortener {
    inner: SxcuDestination,
}

impl SxcuShortener {
    pub fn new(id: impl Into<String>, uploader: CustomUploader, prompt: Arc<dyn Prompt>, rng: Arc<dyn Rng>) -> Self {
        SxcuShortener { inner: SxcuDestination::new(id, uploader, prompt, rng) }
    }
}

impl Shortener for SxcuShortener {
    fn id(&self) -> &str {
        self.inner.id()
    }

    fn shorten(&self, url: &str) -> BoxFuture<'_, Result<String, UploadError>> {
        let url = url.to_owned();
        Box::pin(async move {
            let input = Input { file_name: "", data: &[], mime: "text/plain", text: Some(url.as_str()) };
            let progress: ProgressCallback = Arc::new(|_: u64, _: Option<u64>| {});
            let result = self.inner.exchange(&input, progress).await?;
            result.url.ok_or_else(|| UploadError::BadResponse("the response has no short URL".into()))
        })
    }
}

/// A `.sxcu` URL sharing service: its request carries the URL as `{input}`, and the service does
/// the sharing, so there is no page to open.
pub struct SxcuSharer {
    inner: SxcuDestination,
}

impl SxcuSharer {
    pub fn new(id: impl Into<String>, uploader: CustomUploader, prompt: Arc<dyn Prompt>, rng: Arc<dyn Rng>) -> Self {
        SxcuSharer { inner: SxcuDestination::new(id, uploader, prompt, rng) }
    }
}

impl Sharer for SxcuSharer {
    fn id(&self) -> &str {
        self.inner.id()
    }

    fn share_url(&self, url: &str) -> BoxFuture<'_, Result<Option<String>, UploadError>> {
        let url = url.to_owned();
        Box::pin(async move {
            let input = Input { file_name: "", data: &[], mime: "text/plain", text: Some(url.as_str()) };
            let progress: ProgressCallback = Arc::new(|_: u64, _: Option<u64>| {});
            self.inner.exchange(&input, progress).await?;
            Ok(None)
        })
    }
}

impl fmt::Debug for SxcuSharer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SxcuSharer").field("id", &self.inner.id).finish_non_exhaustive()
    }
}

impl fmt::Debug for SxcuShortener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SxcuShortener").field("id", &self.inner.id).finish_non_exhaustive()
    }
}

/// Transport failures that a retry can fix are `Network`; the rest of the transport errors mean the
/// exchange itself went wrong.
fn http_error(error: HttpError) -> UploadError {
    match error {
        HttpError::Network(message) => UploadError::Network(message),
        HttpError::Timeout => UploadError::Network("timed out".into()),
        other => UploadError::BadResponse(other.to_string()),
    }
}

/// Answers each `{inputbox}` title once for the life of the destination. A cancelled or failed
/// question is asked again next time. `{select}`, `{outputbox}` and confirmations always reach the
/// user.
struct Remembered {
    inner: Arc<dyn Prompt>,
    answers: Mutex<HashMap<String, String>>,
}

impl Remembered {
    fn new(inner: Arc<dyn Prompt>) -> Self {
        Remembered { inner, answers: Mutex::new(HashMap::new()) }
    }
}

impl Prompt for Remembered {
    fn ask_text(&self, title: &str, default: &str) -> BoxFuture<'_, Result<String, PortError>> {
        let title = title.to_owned();
        let default = default.to_owned();
        Box::pin(async move {
            let known = self.answers.lock().unwrap_or_else(PoisonError::into_inner).get(&title).cloned();
            if let Some(answer) = known {
                return Ok(answer);
            }
            let answer = self.inner.ask_text(&title, &default).await?;
            self.answers.lock().unwrap_or_else(PoisonError::into_inner).insert(title, answer.clone());
            Ok(answer)
        })
    }

    fn choose(&self, title: &str, options: &[String]) -> BoxFuture<'_, Result<usize, PortError>> {
        self.inner.choose(title, options)
    }

    fn show_text(&self, title: &str, text: &str) -> BoxFuture<'_, Result<(), PortError>> {
        self.inner.show_text(title, text)
    }

    fn confirm(&self, title: &str, text: &str) -> BoxFuture<'_, Result<bool, PortError>> {
        self.inner.confirm(title, text)
    }
}
