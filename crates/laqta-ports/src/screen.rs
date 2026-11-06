use laqta_domain::Frame;

use crate::{BoxFuture, PortError};

pub trait ScreenSource: Send + Sync {
    /// Exactly one frame per monitor. An adapter that receives one composed image (the Screenshot
    /// portal) splits it into per-monitor frames using the output layout.
    fn capture_all(&self) -> BoxFuture<'_, Result<Vec<Frame>, PortError>>;
}
