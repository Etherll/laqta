//! Start at login (`Autostart`).

use std::path::Path;

use laqta_ports::{Autostart, PortError};

#[derive(Debug, Default)]
pub struct MacAutostart;

impl Autostart for MacAutostart {
    fn is_enabled(&self) -> Result<bool, PortError> {
        Err(PortError::Unsupported("start at login on macOS".into()))
    }

    fn set_enabled(&self, _enabled: bool, _executable: &Path) -> Result<(), PortError> {
        Err(PortError::Unsupported("start at login on macOS".into()))
    }
}
