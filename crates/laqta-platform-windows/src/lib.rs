//! Windows adapters for the ports.
//!
//! Screen capture uses DXGI Desktop Duplication with one D3D11 device per adapter, falling back
//! to GDI `BitBlt`; HDR outputs are tonemapped to sRGB. Transparent window capture uses GDI over
//! backdrops. Hotkeys and the tray share one dedicated shell thread that owns a hidden window.
//! Per-event sounds play through `PlaySound`. Everything here is Windows-only; on other targets the
//! crate is empty. The pure helpers (key codes, window rules, frame patches, HDR math, backdrop
//! alpha, the shutter sound, the Print Screen rule) also build under `test`, so their tests run on Linux.

#[cfg(any(windows, test))]
mod autostart;
#[cfg(any(windows, test))]
mod backdrop;
#[cfg(windows)]
mod cameras;
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
mod key_hook;
#[cfg(any(windows, test))]
mod keys;
#[cfg(windows)]
mod loopback;
#[cfg(any(windows, test))]
mod ocr;
#[cfg(any(windows, test))]
mod patch;
#[cfg(any(windows, test))]
mod print_screen;
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
#[cfg(any(windows, test))]
mod wgc;
#[cfg(windows)]
mod window_ops;
#[cfg(any(windows, test))]
mod window_rules;

#[cfg(windows)]
pub use imp::*;
#[cfg(windows)]
pub use print_screen::{print_screen_for_snipping, restore_print_screen_for_snipping, set_print_screen_for_snipping};
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
        autostart, capture_dxgi, capture_gdi, clipboard, cursor, highlight, hover, input, integration, ocr, secrets, shell, shell_thread,
        stream, transparent, window_ops,
    };

    /// The Windows adapters, ready to hand to `laqta_app::Ports`.
    pub struct WindowsPorts {
        pub screen: Arc<dyn ScreenSource>,
        pub stream: Arc<dyn ScreenStream>,
        /// The native recorder (Windows.Graphics.Capture + Media Foundation), a second `Media` backend beside FFmpeg.
        pub native_recorder: Arc<dyn laqta_ports::Media>,
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
        pub ocr: Arc<dyn laqta_ports::Ocr>,
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
            native_recorder: Arc::new(crate::wgc::NativeRecorder),
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
            ocr: Arc::new(ocr::WindowsOcr),
            highlighter: Arc::new(highlight::WindowsHighlighter::default()),
        }
    }

    /// Makes the process Per-Monitor-V2 DPI aware, so capture rects are physical pixels. Call it
    /// first thing in `main`. Failure (already set, e.g. by a manifest) is ignored.
    pub fn set_dpi_awareness() {
        // SAFETY: plain Win32 call with a predefined constant.
        let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    }

    /// Laqta is a GUI program (no console window of its own). When it is started from a terminal,
    /// this joins that terminal's console so `--version`, `--help` and errors still print there.
    pub fn attach_parent_console() {
        use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
        // SAFETY: plain Win32 call; it fails harmlessly when there is no parent console.
        let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
    }

    /// Stops Windows drawing its own outline and shadow around a borderless popup (a toast, the
    /// recording bar, a border strip), so only what the popup paints shows. `hwnd` is the window's
    /// native handle. Windows 10 has no outline setting; that call fails there and is ignored.
    /// Removes the browser native-messaging registration, for the uninstaller.
    pub fn remove_native_messaging() -> Result<(), PortError> {
        integration::WindowsShellIntegration::default().remove_native_messaging()
    }

    pub fn plain_popup(hwnd: isize) {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::Graphics::Dwm::{
            DWMNCRP_DISABLED, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DWMWA_NCRENDERING_POLICY, DwmSetWindowAttribute,
        };
        let hwnd = HWND(hwnd as *mut core::ffi::c_void);
        let (none, off) = (DWMWA_COLOR_NONE, DWMNCRP_DISABLED.0);
        // SAFETY: each value outlives its call and the size passed is its own. A handle that is
        // no longer a window makes the calls fail, which is ignored.
        unsafe {
            let _ = DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, (&raw const none).cast(), size_of::<u32>() as u32);
            let _ = DwmSetWindowAttribute(hwnd, DWMWA_NCRENDERING_POLICY, (&raw const off).cast(), size_of::<i32>() as u32);
        }
    }

    /// Takes the window `hwnd` off the screen at once and leaves it minimized, then waits until the
    /// desktop has been drawn without it. Minimizing a visible window is animated, and a capture
    /// taken during the animation shows the window; hiding is not animated. Call it on the thread
    /// that owns the window.
    pub fn hide_for_capture(hwnd: isize) {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::Graphics::Dwm::DwmFlush;
        use windows::Win32::UI::WindowsAndMessaging::{SW_HIDE, SW_SHOWMINNOACTIVE, ShowWindow};
        let hwnd = HWND(hwnd as *mut core::ffi::c_void);
        // SAFETY: plain Win32 calls; a handle that is no longer a window makes them fail harmlessly.
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
            let _ = ShowWindow(hwnd, SW_SHOWMINNOACTIVE);
            // One pass may already have been under way; the second one started after the change.
            let _ = DwmFlush();
            let _ = DwmFlush();
        }
    }

    /// Makes the window `hwnd` the foreground window, so it gets the keyboard. Windows refuses a
    /// plain request from a program that is not in the foreground, which Laqta no longer is once
    /// its main window has minimized for a capture. Joining the foreground window's input queue
    /// for the call lifts that refusal. Call it on the thread that owns the window.
    pub fn bring_to_front(hwnd: isize) {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
        use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
        use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow};
        let hwnd = HWND(hwnd as *mut core::ffi::c_void);
        // SAFETY: plain Win32 calls on handles that are only read. Each fails harmlessly when a
        // window is gone, and the two input queues are separated again before returning.
        unsafe {
            let foreground = GetForegroundWindow();
            if foreground == hwnd {
                return;
            }
            let (this, other) = (GetCurrentThreadId(), GetWindowThreadProcessId(foreground, None));
            let joined = other != 0 && other != this && AttachThreadInput(this, other, true).as_bool();
            let _ = SetForegroundWindow(hwnd);
            let _ = SetFocus(Some(hwnd));
            if joined {
                let _ = AttachThreadInput(this, other, false);
            }
        }
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
            Box::pin(blocking::unblock(|| capture_frames(true)))
        }

        fn capture(&self, request: CaptureRequest) -> BoxFuture<'_, Result<Vec<Frame>, PortError>> {
            Box::pin(blocking::unblock(move || {
                let mut frames = capture_frames(request.hdr_tonemap)?;
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

    /// One frame per monitor: DXGI duplication, or GDI when duplication fails. `hdr_tonemap`: see
    /// `CaptureRequest`.
    fn capture_frames(hdr_tonemap: bool) -> Result<Vec<Frame>, PortError> {
        match capture_dxgi::capture(hdr_tonemap) {
            Ok(frames) => Ok(frames),
            Err(e) => {
                tracing::info!("DXGI duplication failed ({e}); falling back to GDI");
                capture_gdi::capture()
            }
        }
    }
}
