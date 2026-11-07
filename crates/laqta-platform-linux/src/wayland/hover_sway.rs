//! sway hover targets (M1 wave 2, task W2d).
use laqta_domain::region::HoverTarget;
use laqta_ports::{BoxFuture, HoverSource, PortError};

#[derive(Debug, Default)]
pub struct SwayHover;

impl HoverSource for SwayHover {
    fn targets(&self) -> BoxFuture<'_, Result<Vec<HoverTarget>, PortError>> {
        Box::pin(async { Err(PortError::Unsupported("sway hover targets: not built yet".into())) })
    }
    fn active_window(&self) -> BoxFuture<'_, Result<Option<HoverTarget>, PortError>> {
        Box::pin(async { Ok(None) })
    }
}
