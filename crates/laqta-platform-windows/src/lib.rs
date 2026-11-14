//! Windows adapters for the ports.
//!
//! Screen capture uses DXGI Desktop Duplication with one D3D11 device per adapter, falling back
//! to Windows.Graphics.Capture and then to GDI `BitBlt`; HDR outputs are tonemapped to sRGB. Transparent window capture uses GDI over
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
pub mod elevated_watch;
#[cfg(any(windows, test))]
pub mod elevation;
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

/// Remembers which monitors' black frames were retaken lately and stayed black, so a screen that really
/// is black does not cost a Windows.Graphics.Capture attempt on every capture.
#[cfg(any(windows, test))]
mod retake_throttle {
    use std::time::{Duration, Instant};

    /// How long a monitor that stayed black is left alone.
    pub(crate) const PAUSE: Duration = Duration::from_secs(2);

    #[derive(Debug)]
    pub(crate) struct Throttle {
        /// The monitors whose last retake gave nothing, and when that ended.
        barren: Vec<(String, Instant)>,
    }

    impl Throttle {
        pub(crate) const fn new() -> Self {
            Throttle { barren: Vec::new() }
        }

        /// Whether a black frame of `monitor` is worth a retake at `now`.
        pub(crate) fn due(&self, monitor: &str, now: Instant) -> bool {
            self.barren.iter().find(|(name, _)| name == monitor).is_none_or(|(_, since)| now.saturating_duration_since(*since) >= PAUSE)
        }

        /// The retake of `monitor`, which ended at `now`, came back black or failed.
        pub(crate) fn barren(&mut self, monitor: &str, now: Instant) {
            match self.barren.iter_mut().find(|(name, _)| name == monitor) {
                Some((_, since)) => *since = now,
                None => self.barren.push((monitor.to_owned(), now)),
            }
        }

        /// The retake of `monitor` gave a picture, so the next black frame gets one too.
        pub(crate) fn fruitful(&mut self, monitor: &str) {
            self.barren.retain(|(name, _)| name != monitor);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        const FIRST: &str = "DISPLAY1";
        const SECOND: &str = "DISPLAY2";

        #[test]
        fn a_monitor_that_stayed_black_is_left_alone_for_two_seconds() {
            let start = Instant::now();
            let mut throttle = Throttle::new();
            assert!(throttle.due(FIRST, start));
            throttle.barren(FIRST, start);
            assert!(!throttle.due(FIRST, start));
            assert!(!throttle.due(FIRST, start + PAUSE - Duration::from_millis(1)));
            assert!(throttle.due(FIRST, start + PAUSE));
        }

        #[test]
        fn each_monitor_has_its_own_pause() {
            let start = Instant::now();
            let mut throttle = Throttle::new();
            throttle.barren(FIRST, start);
            assert!(throttle.due(SECOND, start));
        }

        #[test]
        fn a_later_black_retake_starts_the_pause_again() {
            let start = Instant::now();
            let mut throttle = Throttle::new();
            throttle.barren(FIRST, start);
            throttle.barren(FIRST, start + Duration::from_secs(3));
            assert!(!throttle.due(FIRST, start + Duration::from_secs(4)));
            assert!(throttle.due(FIRST, start + Duration::from_secs(5)));
        }

        #[test]
        fn a_retake_that_gave_a_picture_ends_the_pause() {
            let start = Instant::now();
            let mut throttle = Throttle::new();
            throttle.barren(FIRST, start);
            throttle.fruitful(FIRST);
            assert!(throttle.due(FIRST, start));
        }
    }
}

#[cfg(windows)]
pub use imp::*;
#[cfg(windows)]
pub use print_screen::{print_screen_for_snipping, restore_print_screen_for_snipping, set_print_screen_for_snipping};
#[cfg(windows)]
pub use sound::play_sound;

#[cfg(windows)]
mod imp {
    use std::sync::atomic::{AtomicIsize, Ordering};
    use std::sync::{Arc, Mutex, PoisonError};
    use std::time::Instant;

    use laqta_domain::region::HoverTarget;
    use laqta_domain::{Bitmap, Frame};
    use laqta_ports::{
        BoxFuture, CaptureRequest, Clipboard, HotkeyRegistrar, PortError, ScreenSource, ScreenStream, SecretStore, Shell, Tray,
    };
    use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};

    use crate::{
        autostart, capture_dxgi, capture_gdi, clipboard, cursor, highlight, hover, input, integration, ocr, secrets, shell, shell_thread,
        stream, transparent, wgc, window_ops,
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

    /// The window `remember_foreground` stored, as a raw `HWND` value; 0 when there is none.
    static REMEMBERED_FOREGROUND: AtomicIsize = AtomicIsize::new(0);

    /// Remembers the foreground window, to give the keyboard back to it with `restore_foreground`
    /// when a capture ends. A foreground window that belongs to Laqta's own process (or no
    /// foreground window at all) is skipped, and the window remembered before stays.
    pub fn remember_foreground() {
        use windows::Win32::System::Threading::GetCurrentProcessId;
        use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
        // SAFETY: plain Win32 calls; they only read, and fail harmlessly for a window that is gone.
        unsafe {
            let foreground = GetForegroundWindow();
            if foreground.0.is_null() {
                return;
            }
            let mut owner = 0u32;
            GetWindowThreadProcessId(foreground, Some(&raw mut owner));
            if owner != 0 && owner != GetCurrentProcessId() {
                REMEMBERED_FOREGROUND.store(foreground.0 as isize, Ordering::Release);
            }
        }
    }

    /// Makes the window `remember_foreground` stored the foreground window again, if it still
    /// exists and is not minimized, and forgets it. Call it while Laqta still holds the foreground,
    /// so Windows allows the switch.
    pub fn restore_foreground() {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::System::Threading::GetCurrentProcessId;
        use windows::Win32::UI::WindowsAndMessaging::{
            GetForegroundWindow, GetWindowThreadProcessId, IsIconic, IsWindow, SetForegroundWindow,
        };
        let remembered = REMEMBERED_FOREGROUND.swap(0, Ordering::AcqRel);
        if remembered == 0 {
            return;
        }
        // Only while Laqta still has the keyboard: a window the user clicked meanwhile keeps it.
        // SAFETY: plain Win32 reads.
        let ours = unsafe {
            let mut owner = 0u32;
            GetWindowThreadProcessId(GetForegroundWindow(), Some(&raw mut owner));
            owner == GetCurrentProcessId()
        };
        if !ours {
            return;
        }
        let hwnd = HWND(remembered as *mut core::ffi::c_void);
        // SAFETY: plain Win32 calls on a handle that is only read. A handle that is no longer a
        // window is caught by `IsWindow`; a reused one just gets the foreground request.
        unsafe {
            if IsWindow(Some(hwnd)).as_bool() && !IsIconic(hwnd).as_bool() {
                let _ = SetForegroundWindow(hwnd);
            }
        }
    }

    /// Chooses how the global shortcuts are caught. Off (the default): `RegisterHotKey`, which a game
    /// that takes the keyboard for itself can starve. On: a low-level keyboard hook, which sees the
    /// keys first and takes the ones that match a shortcut. A change applies at once to the
    /// shortcuts that are registered, and to the next registration otherwise. Call it with the
    /// `hotkeys_via_hook` setting before the first registration and whenever it changes.
    pub fn set_hotkeys_via_hook(on: bool) {
        shell_thread::set_via_hook(on);
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
            Box::pin(blocking::unblock(|| capture_frames(CaptureRequest::default())))
        }

        fn capture(&self, request: CaptureRequest) -> BoxFuture<'_, Result<Vec<Frame>, PortError>> {
            Box::pin(blocking::unblock(move || {
                let mut frames = capture_frames(request)?;
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

    /// One frame per monitor: DXGI duplication first. When that fails, or gives a monitor an entirely
    /// black frame (full-screen games and some drivers do that), Windows.Graphics.Capture takes the
    /// frame instead, and GDI is the last resort. A monitor that is black on both stays as DXGI saw it.
    /// `LAQTA_FORCE_WGC=1` skips DXGI. `hdr_tonemap`: see `CaptureRequest`. `show_cursor` only goes to
    /// the debug log, which says once per capture which path was used and what DXGI reported about
    /// the pointer.
    fn capture_frames(request: CaptureRequest) -> Result<Vec<Frame>, PortError> {
        let CaptureRequest { hdr_tonemap, show_cursor, in_series, .. } = request;
        if wgc::still::forced_by_env() {
            tracing::info!("LAQTA_FORCE_WGC is set; skipping DXGI duplication");
        } else {
            match capture_dxgi::capture(hdr_tonemap) {
                Ok(captured) => {
                    tracing::debug!(path = "dxgi", show_cursor, pointer_visible = captured.pointer_visible, "screen capture");
                    let mut frames = captured.frames;
                    // A scrolling capture takes many frames in a row; a retake per step would slow it badly.
                    if !show_cursor && !in_series {
                        retake_hdr_without_pointer(&mut frames, &captured.hdr_white, hdr_tonemap);
                    }
                    retake_black_frames(&mut frames);
                    return Ok(frames);
                }
                Err(e) => tracing::info!("DXGI duplication failed ({e}); trying Windows.Graphics.Capture"),
            }
        }
        match wgc::still::capture_all() {
            Ok(frames) => {
                tracing::debug!(path = "wgc", show_cursor, "screen capture");
                Ok(frames)
            }
            Err(e) => {
                tracing::info!("Windows.Graphics.Capture failed ({e}); falling back to GDI");
                if capture_dxgi::any_hdr_output() {
                    tracing::info!("GDI capture on an HDR output: the colours will not match the screen");
                }
                let frames = capture_gdi::capture()?;
                tracing::debug!(path = "gdi", show_cursor, "screen capture");
                Ok(frames)
            }
        }
    }

    /// Takes each HDR monitor again through Windows.Graphics.Capture with the pointer left out. On an
    /// HDR desktop Windows often draws the pointer into the picture itself, so duplication holds it
    /// whatever `show_cursor` says. A retake that fails keeps the duplication frame.
    fn retake_hdr_without_pointer(frames: &mut [Frame], hdr_white: &[Option<f32>], hdr_tonemap: bool) {
        for (frame, white) in frames.iter_mut().zip(hdr_white) {
            let Some(white) = *white else { continue };
            match wgc::still::capture_monitor(&frame.monitor, hdr_tonemap.then_some(white)) {
                Ok(retaken) if !wgc::still::is_all_black(&retaken.image) => {
                    tracing::debug!(monitor = %frame.monitor.name, "HDR monitor taken again without the pointer");
                    *frame = retaken;
                }
                Ok(_) => tracing::debug!(monitor = %frame.monitor.name, "HDR retake was black; keeping the duplication frame"),
                Err(e) => tracing::info!(monitor = %frame.monitor.name, "HDR retake without the pointer failed ({e})"),
            }
        }
    }

    /// Replaces each entirely black frame with a Windows.Graphics.Capture frame of the same monitor,
    /// when that one has something in it. A monitor that is off or showing black is black in both, and
    /// keeps the frame it had.
    ///
    /// A monitor whose retake came back black or failed is not retaken again for 2 seconds, so a
    /// black screen does not add the wait for Windows.Graphics.Capture to every capture.
    fn retake_black_frames(frames: &mut [Frame]) {
        use crate::retake_throttle::Throttle;
        static THROTTLE: Mutex<Throttle> = Mutex::new(Throttle::new());
        for frame in frames {
            if !wgc::still::is_all_black(&frame.image) {
                continue;
            }
            let name = frame.monitor.name.clone();
            if !THROTTLE.lock().unwrap_or_else(PoisonError::into_inner).due(&name, Instant::now()) {
                tracing::debug!(monitor = %name, "DXGI gave a black frame; it was black in Windows.Graphics.Capture a moment ago too");
                continue;
            }
            let retaken = match wgc::still::capture_monitor(&frame.monitor, None) {
                Ok(retaken) if !wgc::still::is_all_black(&retaken.image) => Some(retaken),
                Ok(_) => {
                    tracing::debug!(monitor = %name, "the monitor is black in Windows.Graphics.Capture too");
                    None
                }
                Err(e) => {
                    tracing::info!(monitor = %name, "DXGI gave a black frame and Windows.Graphics.Capture failed ({e})");
                    None
                }
            };
            let mut throttle = THROTTLE.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(retaken) = retaken {
                tracing::info!(monitor = %name, "DXGI gave a black frame; using Windows.Graphics.Capture for it");
                throttle.fruitful(&name);
                *frame = retaken;
            } else {
                throttle.barren(&name, Instant::now());
            }
        }
    }
}
