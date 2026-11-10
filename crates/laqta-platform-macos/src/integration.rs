//! Finder and browser integration (`ShellIntegration`).

use std::path::Path;

use laqta_ports::{PortError, ShellIntegration};

#[derive(Debug, Default)]
pub struct MacIntegration;

impl ShellIntegration for MacIntegration {
    fn is_installed(&self) -> Result<bool, PortError> {
        Err(PortError::Unsupported("Finder integration on macOS".into()))
    }

    fn install(&self, _executable: &Path) -> Result<(), PortError> {
        Err(PortError::Unsupported("Finder integration on macOS".into()))
    }

    fn uninstall(&self) -> Result<(), PortError> {
        Err(PortError::Unsupported("Finder integration on macOS".into()))
    }

    fn install_native_messaging(&self, _nmh: &Path) -> Result<(), PortError> {
        Err(PortError::Unsupported("browser integration on macOS".into()))
    }
}
