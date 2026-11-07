use laqta_domain::Frame;

use crate::{BoxFuture, PortError};

/// Options a capture honours where the platform can.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CaptureRequest {
    pub show_cursor: bool,
}

pub trait ScreenSource: Send + Sync {
    /// Exactly one frame per monitor. An adapter that receives one composed image (the Screenshot
    /// portal) splits it into per-monitor frames using the output layout.
    fn capture_all(&self) -> BoxFuture<'_, Result<Vec<Frame>, PortError>>;

    /// Like `capture_all`, drawing the cursor into its monitor's frame when asked and supported.
    fn capture(&self, request: CaptureRequest) -> BoxFuture<'_, Result<Vec<Frame>, PortError>> {
        let _ = request;
        self.capture_all()
    }
}
