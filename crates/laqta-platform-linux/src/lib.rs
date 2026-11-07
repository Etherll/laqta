//! Linux adapters for the M0 ports: X11 and Wayland capture, hotkeys, tray, clipboard and shell.
//! Port vocabulary stays Laqta's; X11/Wayland/portal terms stay inside this crate.
#![cfg(target_os = "linux")]

use std::sync::Arc;

use laqta_ports::{Clipboard, HotkeyRegistrar, HoverSource, ScreenSource, Shell, Tray};

mod atspi_elements;
mod clipboard;
mod secrets;
pub mod session;
mod shell;
mod sound;
mod tray;
mod wayland;
mod x11;

pub use secrets::SecretServiceStore;
pub use session::Session;
pub use sound::play_sound;

/// Every port Linux provides in M0, for the session `main` detected.
#[allow(missing_debug_implementations)]
pub struct LinuxPorts {
    pub screen: Arc<dyn ScreenSource>,
    pub clipboard: Arc<dyn Clipboard>,
    pub shell: Arc<dyn Shell>,
    pub hotkeys: Arc<dyn HotkeyRegistrar>,
    pub tray: Arc<dyn Tray>,
    pub hover: Arc<dyn HoverSource>,
    pub secrets: Arc<dyn laqta_ports::SecretStore>,
}

pub fn ports(session: Session) -> LinuxPorts {
    let (screen, hotkeys, hover): (Arc<dyn ScreenSource>, Arc<dyn HotkeyRegistrar>, Arc<dyn HoverSource>) = match session {
        Session::X11 => (Arc::new(x11::capture::X11Screen), Arc::new(x11::hotkeys::X11Hotkeys::default()), Arc::new(x11::hover::X11Hover)),
        Session::Wayland => (
            Arc::new(wayland::WaylandScreen),
            Arc::new(wayland::shortcuts_portal::PortalShortcuts::default()),
            Arc::new(wayland::hover_sway::SwayHover),
        ),
    };
    LinuxPorts {
        screen,
        clipboard: Arc::new(clipboard::ArboardClipboard::default()),
        shell: Arc::new(shell::XdgShell),
        hotkeys,
        tray: Arc::new(tray::SniTray::default()),
        hover: Arc::new(atspi_elements::WithElements::new(hover)),
        secrets: Arc::new(secrets::SecretServiceStore),
    }
}
