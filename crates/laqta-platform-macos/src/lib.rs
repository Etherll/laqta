//! macOS adapters for the ports (ADR 030).
//!
//! Everything here is macOS-only; on other targets the crate is empty. Pure helpers (key codes,
//! coordinate conversion, manifest contents) also build under `test`, so their tests run on Linux.
//! AppKit calls are sent to the main queue from inside the adapter; nothing here uses GPUI.

#[cfg(target_os = "macos")]
mod autostart;
#[cfg(target_os = "macos")]
mod ax;
#[cfg(target_os = "macos")]
mod capture;
#[cfg(target_os = "macos")]
mod clipboard;
#[cfg(target_os = "macos")]
mod cover;
#[cfg(target_os = "macos")]
mod disc;
#[cfg(any(target_os = "macos", test))]
mod displays;
#[cfg(target_os = "macos")]
mod highlight;
#[cfg(target_os = "macos")]
mod hotkeys;
#[cfg(target_os = "macos")]
mod hover;
#[cfg(target_os = "macos")]
mod input;
#[cfg(target_os = "macos")]
mod integration;
#[cfg(any(target_os = "macos", test))]
mod keys;
#[cfg(target_os = "macos")]
mod main_thread;
#[cfg(any(target_os = "macos", test))]
mod manifest;
#[cfg(target_os = "macos")]
mod ocr;
#[cfg(target_os = "macos")]
mod permissions;
#[cfg(target_os = "macos")]
mod secrets;
#[cfg(target_os = "macos")]
mod shell;
#[cfg(target_os = "macos")]
mod sound;
#[cfg(target_os = "macos")]
mod stream;
#[cfg(target_os = "macos")]
mod tray;
#[cfg(target_os = "macos")]
mod window_ops;

#[cfg(target_os = "macos")]
pub use imp::*;
#[cfg(target_os = "macos")]
pub use permissions::{granted as permission_granted, request as request_permission};
#[cfg(target_os = "macos")]
pub use sound::play_sound;

#[cfg(target_os = "macos")]
mod imp {
    use std::sync::Arc;

    use laqta_ports::{
        Autostart, Clipboard, HotkeyRegistrar, HoverSource, InputSynth, Ocr, PointerHighlighter, ScreenSource, ScreenStream, SecretStore,
        Shell, ShellIntegration, Tray, WindowOps,
    };

    use crate::{
        autostart, capture, clipboard, highlight, hotkeys, hover, input, integration, ocr, secrets, shell, stream, tray, window_ops,
    };

    /// The macOS adapters, ready to hand to `laqta_app::Ports`.
    pub struct MacPorts {
        pub screen: Arc<dyn ScreenSource>,
        pub stream: Arc<dyn ScreenStream>,
        pub clipboard: Arc<dyn Clipboard>,
        pub shell: Arc<dyn Shell>,
        pub hotkeys: Arc<dyn HotkeyRegistrar>,
        pub tray: Arc<dyn Tray>,
        pub hover: Arc<dyn HoverSource>,
        pub secrets: Arc<dyn SecretStore>,
        pub window_ops: Arc<dyn WindowOps>,
        pub autostart: Arc<dyn Autostart>,
        pub integration: Arc<dyn ShellIntegration>,
        pub input: Arc<dyn InputSynth>,
        pub ocr: Arc<dyn Ocr>,
        pub highlighter: Arc<dyn PointerHighlighter>,
    }

    impl std::fmt::Debug for MacPorts {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("MacPorts")
        }
    }

    /// A rect in the ports' physical pixels as `[x, y, width, height]` in global points, which are
    /// GPUI's logical pixels, cut to the one display a recording of it covers. `None` when no
    /// display is known.
    pub fn logical_rect(rect: laqta_domain::geometry::PhysRect) -> Option<[f32; 4]> {
        let displays = crate::displays::list();
        crate::displays::to_points_on_display(&displays, rect).map(|r| r.map(|v| v as f32))
    }

    pub fn ports() -> MacPorts {
        MacPorts {
            screen: Arc::new(capture::MacScreen),
            stream: Arc::new(stream::MacStream),
            clipboard: Arc::new(clipboard::MacClipboard),
            shell: Arc::new(shell::MacShell),
            hotkeys: Arc::new(hotkeys::MacHotkeys),
            tray: Arc::new(tray::MacTray),
            hover: Arc::new(hover::MacHover),
            secrets: Arc::new(secrets::Keychain),
            window_ops: Arc::new(window_ops::MacWindowOps),
            autostart: Arc::new(autostart::MacAutostart),
            integration: Arc::new(integration::MacIntegration),
            input: Arc::new(input::MacInput),
            ocr: Arc::new(ocr::VisionOcr),
            highlighter: Arc::new(highlight::MacHighlighter::default()),
        }
    }
}

#[cfg(target_os = "macos")]
pub use cover::cover_screen;
