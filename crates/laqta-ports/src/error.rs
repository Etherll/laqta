#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum PortError {
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("permission denied")]
    PermissionDenied,
    #[error("cancelled")]
    Cancelled,
    /// The service couldn't be reached: nothing listening, no route, a name that doesn't resolve.
    #[error("unreachable: {0}")]
    Unreachable(String),
    /// The service took too long to answer.
    #[error("timed out")]
    TimedOut,
    #[error("{0}")]
    Failed(String),
}
