//! The menu bar item (`Tray`).

use laqta_ports::{ActionCallback, PortError, Tray, TrayMenu};

#[derive(Debug, Default)]
pub struct MacTray;

impl Tray for MacTray {
    fn show(&self, _menu: TrayMenu, _on_select: ActionCallback) -> Result<(), PortError> {
        Err(PortError::Unsupported("the menu bar item on macOS".into()))
    }
}
