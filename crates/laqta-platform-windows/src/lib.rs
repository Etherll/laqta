//! Windows adapters for the M0 ports (docs/plan/M0.md §5.3, §5.4).
//!
//! Screen capture uses DXGI Desktop Duplication with one D3D11 device per adapter, falling back
//! to GDI `BitBlt`; HDR outputs are tonemapped to sRGB. Transparent window capture uses GDI over
//! backdrops. Hotkeys and the tray share one dedicated shell thread that owns a hidden window.
//! Per-event sounds play through `PlaySound`. Everything here is Windows-only; on other targets the
//! crate is empty. The pure helpers (key codes, window rules, frame patches, HDR math, backdrop
//! alpha, the shutter sound) also build under `test`, so their tests run on Linux.

#[cfg(any(windows, test))]
mod autostart;
#[cfg(any(windows, test))]
mod backdrop;
#[cfg(windows)]
mod capture_dxgi;
#[cfg(windows)]
mod capture_gdi;
#[cfg(windows)]
mod clipboard;
#[cfg(windows)]
mod cursor;
#[cfg(any(windows, test))]
mod hdr;
#[cfg(any(windows, test))]
mod highlight;
#[cfg(windows)]
mod hover;
#[cfg(any(windows, test))]
mod input;
#[cfg(any(windows, test))]
mod integration;
#[cfg(any(windows, test))]
mod keys;
#[cfg(windows)]
mod loopback;
#[cfg(any(windows, test))]
mod patch;
#[cfg(any(windows, test))]
mod secrets;
#[cfg(windows)]
mod shell;
#[cfg(windows)]
mod shell_thread;
#[cfg(any(windows, test))]
mod shutter;
#[cfg(windows)]
mod sound;
#[cfg(any(windows, test))]
mod stream;
#[cfg(windows)]
mod transparent;
#[cfg(windows)]
mod window_ops;
#[cfg(any(windows, test))]
mod window_rules;

#[cfg(windows)]
pub use imp::*;
#[cfg(windows)]
pub use sound::play_sound;

#[cfg(windows)]
mod imp {
    use std::sync::Arc;

    use laqta_domain::region::HoverTarget;
    use laqta_domain::{Bitmap, Frame};
    use laqta_ports::{
        BoxFuture, CaptureRequest, Clipboard, HotkeyRegistrar, PortError, ScreenSource, ScreenStream, SecretStore, Shell, Tray,
    };
    use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};

    use crate::{
        autostart, capture_dxgi, capture_gdi, clipboard, cursor, highlight, hover, input, integration, secrets, shell, shell_thread,
        stream, transparent, window_ops,
    };

    /// The Windows adapters, ready to hand to `laqta_app::Ports`.
    pub struct WindowsPorts {
        pub screen: Arc<dyn ScreenSource>,
        pub stream: Arc<dyn ScreenStream>,
        pub clipboard: Arc<dyn Clipboard>,
        pub shell: Arc<dyn Shell>,
        pub hotkeys: Arc<dyn HotkeyRegistrar>,
        pub tray: Arc<dyn Tray>,
        pub hover: Arc<dyn laqta_ports::HoverSource>,
        pub secrets: Arc<dyn SecretStore>,
        pub window_ops: Arc<dyn laqta_ports::WindowOps>,
        pub autostart: Arc<dyn laqta_ports::Autostart>,
        pub integration: Arc<dyn laqta_ports::ShellIntegration>,
        pub input: Arc<dyn laqta_ports::InputSynth>,
        pub highlighter: Arc<dyn laqta_ports::PointerHighlighter>,
    }

    impl std::fmt::Debug for WindowsPorts {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("WindowsPorts")
        }
    }

    pub fn ports() -> WindowsPorts {
        WindowsPorts {
            screen: Arc::new(WindowsScreen),
            stream: Arc::new(stream::WindowsStream),
            clipboard: Arc::new(clipboard::WindowsClipboard),
            shell: Arc::new(shell::WindowsShell),
            hotkeys: Arc::new(shell_thread::WindowsHotkeys),
            tray: Arc::new(shell_thread::WindowsTray),
            hover: Arc::new(hover::WindowsHover),
            secrets: Arc::new(secrets::CredentialStore),
            window_ops: Arc::new(window_ops::WindowsWindowOps),
            autostart: Arc::new(autostart::WindowsAutostart::default()),
            integration: Arc::new(integration::WindowsShellIntegration::default()),
            input: Arc::new(input::WindowsInput),
            highlighter: Arc::new(highlight::WindowsHighlighter::default()),
        }
    }

    /// Makes the process Per-Monitor-V2 DPI aware, so capture rects are physical pixels. Call it
    /// first thing in `main`. Failure (already set, e.g. by a manifest) is ignored.
    pub fn set_dpi_awareness() {
        // SAFETY: plain Win32 call with a predefined constant.
        let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    }

    /// Removes the tray icon and stops the shell thread. Call it before the process exits, or
    /// Explorer keeps a ghost icon until the pointer passes over it.
    pub fn shutdown() {
        shell_thread::shutdown();
    }

    /// Captures with the GDI fallback only. For the contract tests.
    #[doc(hidden)]
    pub fn capture_with_gdi() -> Result<Vec<Frame>, PortError> {
        capture_gdi::capture()
    }

    #[derive(Debug)]
    struct WindowsScreen;

    impl ScreenSource for WindowsScreen {
        fn capture_all(&self) -> BoxFuture<'_, Result<Vec<Frame>, PortError>> {
            Box::pin(blocking::unblock(capture_frames))
        }

        fn capture(&self, request: CaptureRequest) -> BoxFuture<'_, Result<Vec<Frame>, PortError>> {
            Box::pin(blocking::unblock(move || {
                let mut frames = capture_frames()?;
                if request.show_cursor {
                    cursor::draw_onto(&mut frames);
                }
                Ok(frames)
            }))
        }

        fn capture_window(&self, target: &HoverTarget, request: CaptureRequest) -> BoxFuture<'_, Result<Bitmap, PortError>> {
            let target = target.clone();
            Box::pin(blocking::unblock(move || transparent::capture(&target, request.show_cursor)))
        }
    }

    /// One frame per monitor: DXGI duplication, or GDI when duplication fails.
    fn capture_frames() -> Result<Vec<Frame>, PortError> {
        match capture_dxgi::capture() {
            Ok(frames) => Ok(frames),
            Err(e) => {
                tracing::info!("DXGI duplication failed ({e}); falling back to GDI");
                capture_gdi::capture()
            }
        }
    }
}
