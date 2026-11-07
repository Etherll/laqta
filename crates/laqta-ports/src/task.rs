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
    StatusChanged {
        id: TaskId,
        status: TaskStatus,
    },
    Saved {
        id: TaskId,
        path: PathBuf,
    },
    Copied {
        id: TaskId,
    },
    CopyFailed {
        id: TaskId,
        reason: String,
    },
    Failed {
        id: TaskId,
        error: AppError,
    },
    /// M1 after-capture steps.
    ThumbnailSaved {
        id: TaskId,
        path: PathBuf,
    },
    /// The file itself (as a file list) or its path (as text) went to the clipboard.
    FileCopied {
        id: TaskId,
    },
    PathCopied {
        id: TaskId,
        path: PathBuf,
    },
    Revealed {
        id: TaskId,
        path: PathBuf,
    },
    Notified {
        id: TaskId,
    },
    Deleted {
        id: TaskId,
        path: PathBuf,
    },
    /// The save was skipped because the file exists (`FileExistsAction::Skip`).
    SaveSkipped {
        id: TaskId,
        path: PathBuf,
    },
    /// A step that failed without failing the task (reveal, toast, thumbnail, …).
    StepFailed {
        id: TaskId,
        step: &'static str,
        reason: String,
    },
}

impl TaskEvent {
    pub fn id(&self) -> TaskId {
        match self {
            TaskEvent::StatusChanged { id, .. }
            | TaskEvent::Saved { id, .. }
            | TaskEvent::Copied { id }
            | TaskEvent::CopyFailed { id, .. }
            | TaskEvent::Failed { id, .. }
            | TaskEvent::ThumbnailSaved { id, .. }
            | TaskEvent::FileCopied { id }
            | TaskEvent::PathCopied { id, .. }
            | TaskEvent::Revealed { id, .. }
            | TaskEvent::Notified { id }
            | TaskEvent::Deleted { id, .. }
            | TaskEvent::SaveSkipped { id, .. }
            | TaskEvent::StepFailed { id, .. } => *id,
        }
    }
}
