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
    /// M2 upload phase.
    UploadStarted { id: TaskId, destination: String },
    Progress { id: TaskId, sent: u64, total: Option<u64> },
    UploadRetry { id: TaskId, attempt: u32, reason: String },
    Uploaded { id: TaskId, url: String, thumbnail_url: Option<String>, deletion_url: Option<String> },
    Shortened { id: TaskId, url: String },
    UrlCopied { id: TaskId, text: String },
    UrlOpened { id: TaskId, url: String },
    Shared { id: TaskId, url: String },
    /// Asks the UI to show the after-upload window or a QR code for the URL.
    ShowUrl { id: TaskId, url: String, qr: bool },
    /// M4 recording session (the id is the recording's own task id).
    RecordingCountdown { id: TaskId, seconds_left: u32 },
    RecordingStarted { id: TaskId, area: laqta_domain::geometry::PhysRect },
    RecordingPaused { id: TaskId },
    RecordingResumed { id: TaskId },
    RecordingTick { id: TaskId, elapsed_secs: u32 },
    RecordingStopped { id: TaskId, path: PathBuf },
    RecordingFailed { id: TaskId, error: AppError, partial: Option<PathBuf> },
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
            | TaskEvent::StepFailed { id, .. }
            | TaskEvent::UploadStarted { id, .. }
            | TaskEvent::Progress { id, .. }
            | TaskEvent::UploadRetry { id, .. }
            | TaskEvent::Uploaded { id, .. }
            | TaskEvent::Shortened { id, .. }
            | TaskEvent::UrlCopied { id, .. }
            | TaskEvent::UrlOpened { id, .. }
            | TaskEvent::Shared { id, .. }
            | TaskEvent::ShowUrl { id, .. }
            | TaskEvent::RecordingCountdown { id, .. }
            | TaskEvent::RecordingStarted { id, .. }
            | TaskEvent::RecordingPaused { id }
            | TaskEvent::RecordingResumed { id }
            | TaskEvent::RecordingTick { id, .. }
            | TaskEvent::RecordingStopped { id, .. }
            | TaskEvent::RecordingFailed { id, .. } => *id,
        }
    }
}
