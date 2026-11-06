#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum PortError {
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("permission denied")]
    PermissionDenied,
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    Failed(String),
}
