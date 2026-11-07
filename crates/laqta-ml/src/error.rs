use std::path::PathBuf;

use laqta_http::HttpError;
use laqta_ports::PortError;

#[derive(Debug, thiserror::Error)]
pub enum MlError {
    #[error("unknown model `{0}`")]
    UnknownModel(String),
    #[error("model `{0}` is not installed")]
    NotInstalled(String),
    #[error("no installed OCR model reads `{0}`")]
    NoModelForLanguage(String),
    #[error("model `{0}` has no pinned SHA-256 yet (\"pending-owner\"), so it cannot be downloaded")]
    PendingHash(String),
    #[error("unknown model family `{0}`")]
    UnknownFamily(String),
    #[error("the ONNX Runtime library was not found at {}", .0.display())]
    OrtLibraryMissing(PathBuf),
    #[error("the model runtime is not loaded; call `Runtime::ensure_ort` first")]
    RuntimeNotLoaded,
    #[error("ONNX Runtime: {0}")]
    Ort(String),
    #[error("model does not match family `{family}`: {detail}")]
    Shape { family: String, detail: String },
    #[error("inference failed: {0}")]
    Inference(String),
    #[error("catalog: {0}")]
    Catalog(String),
    #[error("i/o: {0}")]
    Io(String),
    #[error("download failed: {0}")]
    Download(HttpError),
    #[error("cancelled")]
    Cancelled,
}

impl From<HttpError> for MlError {
    fn from(error: HttpError) -> Self {
        match error {
            HttpError::Cancelled => MlError::Cancelled,
            other => MlError::Download(other),
        }
    }
}

impl From<MlError> for PortError {
    fn from(error: MlError) -> Self {
        match error {
            MlError::Cancelled => PortError::Cancelled,
            other => PortError::Failed(other.to_string()),
        }
    }
}
