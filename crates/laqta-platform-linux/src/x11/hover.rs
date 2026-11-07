//! X11 hover targets (M1 wave 2, task W2b).
use laqta_domain::region::HoverTarget;
use laqta_ports::{BoxFuture, HoverSource, PortError};

use super::windows;

#[derive(Debug, Default)]
pub struct X11Hover;

impl HoverSource for X11Hover {
    fn targets(&self) -> BoxFuture<'_, Result<Vec<HoverTarget>, PortError>> {
        Box::pin(blocking::unblock(windows::targets))
    }
    fn active_window(&self) -> BoxFuture<'_, Result<Option<HoverTarget>, PortError>> {
        Box::pin(blocking::unblock(windows::active_window))
    }
}
