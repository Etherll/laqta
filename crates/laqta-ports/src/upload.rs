//! Uploading (ADR 014): destinations, shorteners, sharers, prompts and secrets.

use std::path::PathBuf;

use crate::{BoxFuture, PortError};

/// What is being uploaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UploadKind {
    Image,
    Text,
    File,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UploadData {
    Bytes(Vec<u8>),
    /// A file on disk; adapters stream it.
    Path(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadRequest {
    /// File name sent to the host, e.g. `Laqta_2026-11-07_14-03-22.png`.
    pub file_name: String,
    pub kind: UploadKind,
    pub data: UploadData,
    pub mime: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UploadResult {
    pub url: Option<String>,
    pub thumbnail_url: Option<String>,
    pub deletion_url: Option<String>,
    /// The raw response body, for `$result` and error reports.
    pub response: String,
}

/// Upload errors carry how a retry should treat them.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum UploadError {
    #[error("network: {0}")]
    Network(String),
    #[error("server error {status}: {body}")]
    Server { status: u16, body: String },
    #[error("authentication: {0}")]
    Auth(String),
    #[error("quota: {0}")]
    Quota(String),
    #[error("bad response: {0}")]
    BadResponse(String),
    #[error("cancelled")]
    Cancelled,
    #[error("unsupported: {0}")]
    Unsupported(String),
}

impl UploadError {
    /// Network errors, 5xx and 429 are worth retrying (risk R7).
    pub fn is_retryable(&self) -> bool {
        match self {
            UploadError::Network(_) => true,
            UploadError::Server { status, .. } => *status >= 500 || *status == 429,
            _ => false,
        }
    }
}

impl From<UploadError> for crate::AppError {
    fn from(e: UploadError) -> Self {
        use crate::AppError;
        match e {
            UploadError::Network(s) => AppError::Network(s),
            UploadError::Server { status, body } => AppError::Network(format!("HTTP {status}: {body}")),
            UploadError::Auth(s) => AppError::Auth(s),
            UploadError::Quota(s) => AppError::Quota(s),
            UploadError::BadResponse(s) => AppError::Network(format!("bad response: {s}")),
            UploadError::Cancelled => AppError::Cancelled,
            UploadError::Unsupported(s) => AppError::Unsupported(s),
        }
    }
}

/// Upload progress: bytes sent so far and the total, if known.
pub type ProgressCallback = std::sync::Arc<dyn Fn(u64, Option<u64>) + Send + Sync>;

pub trait Destination: Send + Sync {
    /// A stable id such as `imgur` or `sxcu:catbox`.
    fn id(&self) -> &str;
    fn name(&self) -> &str;
    fn upload(&self, request: UploadRequest, progress: ProgressCallback) -> BoxFuture<'_, Result<UploadResult, UploadError>>;
}

pub trait Shortener: Send + Sync {
    fn id(&self) -> &str;
    fn shorten(&self, url: &str) -> BoxFuture<'_, Result<String, UploadError>>;
}

/// Sharing services turn a URL into a page to open (Reddit, email, …).
pub trait Sharer: Send + Sync {
    fn id(&self) -> &str;
    fn share_url(&self, url: &str) -> Result<String, UploadError>;
}

/// Questions for the user (custom-uploader `{select}`, `{inputbox}`, the before-upload window).
pub trait Prompt: Send + Sync {
    fn ask_text(&self, title: &str, default: &str) -> BoxFuture<'_, Result<String, PortError>>;
    fn choose(&self, title: &str, options: &[String]) -> BoxFuture<'_, Result<usize, PortError>>;
    fn show_text(&self, title: &str, text: &str) -> BoxFuture<'_, Result<(), PortError>>;
    fn confirm(&self, title: &str, text: &str) -> BoxFuture<'_, Result<bool, PortError>>;
}

/// OS credential storage. `Unsupported` when no keyring exists; callers then keep the secret in
/// memory for the session.
pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<String>, PortError>;
    fn set(&self, key: &str, secret: &str) -> Result<(), PortError>;
    fn delete(&self, key: &str) -> Result<(), PortError>;
}
