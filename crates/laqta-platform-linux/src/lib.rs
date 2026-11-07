//! Linux adapters for the M0 ports: X11 and Wayland capture, hotkeys, tray, clipboard and shell.
//! Port vocabulary stays Laqta's; X11/Wayland/portal terms stay inside this crate.
#![cfg(target_os = "linux")]

use std::sync::Arc;

use laqta_ports::{Clipboard, HotkeyRegistrar, ScreenSource, Shell, Tray};

mod clipboard;
pub mod session;
mod shell;
mod tray;
mod wayland;
mod x11;

pub use session::Session;

/// Every port Linux provides in M0, for the session `main` detected.
#[allow(missing_debug_implementations)]
pub struct LinuxPorts {
    pub screen: Arc<dyn ScreenSource>,
    pub clipboard: Arc<dyn Clipboard>,
    pub shell: Arc<dyn Shell>,
    pub hotkeys: Arc<dyn HotkeyRegistrar>,
    pub tray: Arc<dyn Tray>,
}

pub fn ports(session: Session) -> LinuxPorts {
    let (screen, hotkeys): (Arc<dyn ScreenSource>, Arc<dyn HotkeyRegistrar>) = match session {
        Session::X11 => (Arc::new(x11::capture::X11Screen), Arc::new(x11::hotkeys::X11Hotkeys::default())),
        Session::Wayland => (Arc::new(wayland::WaylandScreen), Arc::new(wayland::shortcuts_portal::PortalShortcuts::default())),
    };
    LinuxPorts {
        screen,
        clipboard: Arc::new(clipboard::ArboardClipboard::default()),
        shell: Arc::new(shell::XdgShell),
        hotkeys,
        tray: Arc::new(tray::SniTray::default()),
    }
}
