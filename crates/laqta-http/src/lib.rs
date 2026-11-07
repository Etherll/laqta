//! The HTTP library (ADR 007): one Tokio runtime on its own thread, started on the first request,
//! with one shared `reqwest` client. Results come back over channels, so callers on any executor
//! can await them. The request and response types below are the contract for the other crates.
#![forbid(unsafe_code)]

mod client;
mod fetch;
mod loopback;
mod runtime;

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

/// The future that resolves to the request target of an OAuth redirect.
type Redirect = Pin<Box<dyn Future<Output = Result<String, HttpError>> + Send>>;

/// Progress as (bytes so far, total when known). Callbacks run on the HTTP runtime thread, so they
/// must return quickly and must not block.
type Progress = Arc<dyn Fn(u64, Option<u64>) + Send + Sync>;

/// An HTTP method.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

/// One part of a multipart body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    Text { name: String, value: String },
    File { name: String, file_name: String, mime: String, data: Vec<u8> },
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Body {
    #[default]
    Empty,
    Bytes { data: Vec<u8>, content_type: String },
    Form(Vec<(String, String)>),
    Multipart(Vec<Part>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub method: Method,
    pub url: String,
    /// Query parameters appended to the URL (percent-encoded by the library).
    pub query: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub body: Body,
    /// Overall timeout in seconds (0 = the client default, 120 s).
    pub timeout_secs: u64,
}

impl Request {
    pub fn new(method: Method, url: impl Into<String>) -> Self {
        Request { method, url: url.into(), query: Vec::new(), headers: Vec::new(), body: Body::Empty, timeout_secs: 0 }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    /// The final URL after redirects.
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    /// Case-insensitive header lookup.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HttpError {
    #[error("network: {0}")]
    Network(String),
    #[error("timed out")]
    Timeout,
    #[error("cancelled")]
    Cancelled,
    #[error("invalid request: {0}")]
    Invalid(String),
    #[error("checksum mismatch: expected {expected}, got {actual}")]
    Checksum { expected: String, actual: String },
    #[error("i/o: {0}")]
    Io(String),
}

/// Sends a request on the shared runtime. Non-2xx statuses are NOT errors (callers inspect status).
pub async fn send(request: Request) -> Result<Response, HttpError> {
    send_request(request, None).await
}

/// Same, reporting upload progress (bytes sent, total) for Bytes and Multipart bodies.
///
/// A multipart body reports its start and its completion only: reqwest does not expose the encoded
/// length, so the total is the size of the parts' data.
pub async fn send_with_progress(
    request: Request,
    progress: Arc<dyn Fn(u64, Option<u64>) + Send + Sync>,
) -> Result<Response, HttpError> {
    send_request(request, Some(progress)).await
}

async fn send_request(request: Request, progress: Option<Progress>) -> Result<Response, HttpError> {
    let rx = runtime::dispatch(move |client| async move {
        client::execute(&client, request, progress).await
    })?;
    runtime::finish(rx).await
}

/// Downloads `url` to `dest` through `<dest>.part`, then renames it into place once the SHA-256
/// (lowercase hex, `sha256_hex`) matches. On a mismatch the download is deleted and `Checksum` is
/// returned. `cancel` is polled between chunks and while waiting for data; a cancelled download
/// returns `Cancelled`. A download that receives no data for 60 s returns `Timeout`. Dropping the
/// future also stops the transfer and leaves `<dest>.part` behind.
pub async fn fetch_verified(
    url: &str,
    dest: &Path,
    sha256_hex: &str,
    progress: Arc<dyn Fn(u64, Option<u64>) + Send + Sync>,
    cancel: Arc<AtomicBool>,
) -> Result<(), HttpError> {
    if !fetch::is_sha256_hex(sha256_hex) {
        return Err(HttpError::Invalid(format!(
            "sha256 must be 64 hex digits, got {sha256_hex:?}"
        )));
    }
    fetch::fetch(
        url.to_owned(),
        dest.to_owned(),
        sha256_hex.to_ascii_lowercase(),
        progress,
        cancel,
    )
    .await
}

/// Runs a future on the HTTP runtime and returns its output to the caller's executor. It is used by
/// SFTP and the OAuth loopback listener, which need Tokio.
///
/// # Panics
///
/// Panics if the runtime cannot start, or if `future` panics on the runtime.
pub async fn run<F>(future: F) -> F::Output
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let rx = runtime::dispatch(move |_client| future).expect("laqta-http runtime failed to start");
    rx.await.expect("laqta-http task ended without a result")
}

/// Listens on 127.0.0.1:0 for one HTTP GET (the OAuth redirect), replies with `reply_html`, and
/// returns (port, future resolving to the request path+query). Dropping the future stops the
/// listener.
pub async fn loopback_once(reply_html: String) -> Result<(u16, Redirect), HttpError> {
    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").map_err(|err| HttpError::Io(err.to_string()))?;
    listener
        .set_nonblocking(true)
        .map_err(|err| HttpError::Io(err.to_string()))?;
    let port = listener
        .local_addr()
        .map_err(|err| HttpError::Io(err.to_string()))?
        .port();
    let rx = runtime::dispatch(move |_client| loopback::serve_once(listener, reply_html))?;
    Ok((port, Box::pin(runtime::finish(rx))))
}

/// True while the runtime thread is alive (for tests and the idle check).
pub fn runtime_alive() -> bool {
    runtime::alive()
}

/// Shortens or lengthens the idle time before the runtime is released. Tests use it; the default
/// is 60 s.
#[doc(hidden)]
pub fn set_release_after_for_tests(after: Duration) {
    runtime::set_release_after(after);
}
