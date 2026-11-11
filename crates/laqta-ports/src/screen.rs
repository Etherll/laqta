use laqta_domain::region::HoverTarget;
use laqta_domain::{Bitmap, Frame};

use crate::{BoxFuture, PortError};

/// Options a capture honours where the platform can.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureRequest {
    pub show_cursor: bool,
    /// HDR screens: Laqta tonemaps the HDR desktop to SDR itself (true), or takes the SDR copy the
    /// OS makes (false). Only the Windows adapter reads it.
    pub hdr_tonemap: bool,
}

impl Default for CaptureRequest {
    fn default() -> Self {
        CaptureRequest { show_cursor: false, hdr_tonemap: true }
    }
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

    /// Captures one window by itself, keeping its transparency (rounded corners, shadows) where
    /// the platform composites windows with alpha (Windows DWM, X11 with a compositor). The result is
    /// straight-alpha and sized to `target.rect`.
    fn capture_window(&self, target: &HoverTarget, request: CaptureRequest) -> BoxFuture<'_, Result<Bitmap, PortError>> {
        let _ = (target, request);
        Box::pin(async { Err(PortError::Unsupported("transparent window capture".into())) })
    }
}
