//! Screenshots (`ScreenSource`) through ScreenCaptureKit.

use laqta_domain::Frame;
use laqta_ports::{BoxFuture, PortError, ScreenSource};

#[derive(Debug, Default)]
pub struct MacScreen;

impl ScreenSource for MacScreen {
    fn capture_all(&self) -> BoxFuture<'_, Result<Vec<Frame>, PortError>> {
        Box::pin(async { Err(PortError::Unsupported("screen capture on macOS".into())) })
    }
}
