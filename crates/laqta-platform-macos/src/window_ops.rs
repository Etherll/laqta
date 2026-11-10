//! Window tools (`WindowOps`) through the Accessibility API.

use laqta_domain::region::HoverTarget;
use laqta_ports::{PortError, WindowOps};

#[derive(Debug, Default)]
pub struct MacWindowOps;

impl WindowOps for MacWindowOps {
    fn set_borderless(&self, _window: &HoverTarget, _borderless: bool) -> Result<(), PortError> {
        Err(PortError::Unsupported("borderless windows on macOS".into()))
    }

    fn set_topmost(&self, _window: &HoverTarget, _topmost: bool) -> Result<(), PortError> {
        Err(PortError::Unsupported("always-on-top windows on macOS".into()))
    }

    fn inspect(&self, _window: &HoverTarget) -> Result<Vec<(String, String)>, PortError> {
        Err(PortError::Unsupported("window details on macOS".into()))
    }
}
