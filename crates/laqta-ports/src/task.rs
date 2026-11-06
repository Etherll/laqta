//! Types carried by task events, shared with laqta-library and laqta-ui.

use std::path::PathBuf;

use crate::PortError;

#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum AppError {
    #[error("cancelled")]
    Cancelled,
    #[error("permission denied")]
    PermissionDenied,
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("i/o: {0}")]
    Io(String),
    #[error("network: {0}")]
    Network(String),
    #[error("authentication: {0}")]
    Auth(String),
    #[error("quota: {0}")]
    Quota(String),
    #[error("internal error: {0}")]
    Panic(String),
}

impl From<PortError> for AppError {
    fn from(e: PortError) -> Self {
        match e {
            PortError::Unsupported(s) => AppError::Unsupported(s),
            PortError::PermissionDenied => AppError::PermissionDenied,
            PortError::Cancelled => AppError::Cancelled,
            PortError::Failed(s) => AppError::Io(s),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TaskId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskStatus {
    InQueue,
    Preparing,
    Working,
    Stopping,
    Stopped,
    Failed,
    Completed,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum TaskEvent {
    StatusChanged { id: TaskId, status: TaskStatus },
    Saved { id: TaskId, path: PathBuf },
    Copied { id: TaskId },
    CopyFailed { id: TaskId, reason: String },
    Failed { id: TaskId, error: AppError },
}
