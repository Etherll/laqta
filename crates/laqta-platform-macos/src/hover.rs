//! Hover targets (`HoverSource`): windows, then elements.

use laqta_domain::region::HoverTarget;
use laqta_ports::{BoxFuture, HoverSource, PortError};

#[derive(Debug, Default)]
pub struct MacHover;

impl HoverSource for MacHover {
    fn targets(&self) -> BoxFuture<'_, Result<Vec<HoverTarget>, PortError>> {
        Box::pin(async { Err(PortError::Unsupported("hover targets on macOS".into())) })
    }

    fn active_window(&self) -> BoxFuture<'_, Result<Option<HoverTarget>, PortError>> {
        Box::pin(async { Err(PortError::Unsupported("the active window on macOS".into())) })
    }
}
