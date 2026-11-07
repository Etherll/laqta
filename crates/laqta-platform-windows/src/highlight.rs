//! The mouse highlighter on Windows (M5 D9): a layered, click-through, top-most popup that shows a
//! disc at the pointer.
//!
//! A thread owns the popup while the highlighter is on. Each tick it reads the pointer, moves the
//! popup there and re-asserts the top-most order, then drains the thread's message queue. The disc
//! is drawn once into a premultiplied 32-bit DIB and pushed with `UpdateLayeredWindow`. The pixel and
//! position maths is plain data, so its tests run on every target.

/// The disc's diameter in pixels. The popup is a square this size.
const DIAMETER: u32 = 48;
/// The ring's width in pixels, measured inwards from the disc's edge.
const RING_WIDTH: f64 = 2.0;
/// The fill's alpha: 0.35 of 255, rounded.
const FILL_ALPHA: u8 = 89;
const FILL_RGB: (u8, u8, u8) = (255, 255, 0);
const RING_RGB: (u8, u8, u8) = (255, 165, 0);
#[cfg(windows)]
pub use imp::WindowsHighlighter;

/// A premultiplied ARGB32 pixel: each colour channel is scaled by the alpha. Read as a little-endian
/// `u32`, a 32-bit DIB's bytes (B, G, R, A) give the same value.
fn premultiplied(alpha: u8, (red, green, blue): (u8, u8, u8)) -> u32 {
    let scale = |channel: u8| (u32::from(channel) * u32::from(alpha) + 127) / 255;
    (u32::from(alpha) << 24) | (scale(red) << 16) | (scale(green) << 8) | scale(blue)
}

/// The disc as premultiplied pixels, top row first. Pixels outside the circle are clear, the outer
/// `RING_WIDTH` pixels are the ring, and the rest is the fill. A pixel is inside when its centre is.
fn disc_pixels(diameter: u32) -> Vec<u32> {
    let radius = f64::from(diameter) / 2.0;
    let ring_inner = radius - RING_WIDTH;
    let fill = premultiplied(FILL_ALPHA, FILL_RGB);
    let ring = premultiplied(u8::MAX, RING_RGB);
    (0..diameter * diameter)
        .map(|index| {
            let dx = f64::from(index % diameter) + 0.5 - radius;
            let dy = f64::from(index / diameter) + 0.5 - radius;
            let distance_sq = dx * dx + dy * dy;
            if distance_sq > radius * radius {
                0
            } else if distance_sq >= ring_inner * ring_inner {
                ring
            } else {
                fill
            }
        })
        .collect()
}

/// The top-left corner that centres a square of `size` on the pointer.
fn window_origin(pointer: (i32, i32), size: i32) -> (i32, i32) {
    (pointer.0 - size / 2, pointer.1 - size / 2)
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard, mpsc};
    use std::thread::JoinHandle;
    use std::time::Duration;

    use laqta_ports::{PointerHighlighter, PortError};
    use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CreateCompatibleDC, CreateDIBSection,
        DIB_RGB_COLORS, DeleteDC, DeleteObject, HBITMAP, HDC, SelectObject,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos, HWND_TOPMOST, MSG, PM_REMOVE, PeekMessageW,
        RegisterClassW, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOSIZE, SetWindowPos, ShowWindow, TranslateMessage, ULW_ALPHA,
        UpdateLayeredWindow, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
    };
    use windows::core::w;

    use super::{DIAMETER, disc_pixels, window_origin};

    /// The pointer is read this often while the highlighter is on.
    const TICK: Duration = Duration::from_millis(16);
    /// How long `set_enabled(true)` waits for the popup thread to report back.
    const START_TIMEOUT: Duration = Duration::from_secs(5);

    /// The mouse highlighter. The popup lives on its own thread while the highlighter is on.
    #[derive(Debug, Default)]
    pub struct WindowsHighlighter {
        running: Mutex<Option<Running>>,
    }

    impl WindowsHighlighter {
        fn lock(&self) -> MutexGuard<'_, Option<Running>> {
            self.running.lock().unwrap_or_else(|p| p.into_inner())
        }
    }

    impl PointerHighlighter for WindowsHighlighter {
        fn set_enabled(&self, on: bool) -> Result<(), PortError> {
            let mut running = self.lock();
            if let Some(current) = running.take() {
                // A thread that has exited counts as off, so turning on restarts it.
                if on && !current.thread.is_finished() {
                    *running = Some(current);
                    return Ok(());
                }
                current.stop();
            }
            if on {
                *running = Some(Running::start()?);
            }
            Ok(())
        }

        fn is_enabled(&self) -> bool {
            self.lock().as_ref().is_some_and(|running| !running.thread.is_finished())
        }
    }

    impl Drop for WindowsHighlighter {
        fn drop(&mut self) {
            let _ = self.set_enabled(false);
        }
    }

    /// The thread that owns the popup while the highlighter is on.
    #[derive(Debug)]
    struct Running {
        flag: Arc<AtomicBool>,
        thread: JoinHandle<()>,
    }

    impl Running {
        fn start() -> Result<Self, PortError> {
            let flag = Arc::new(AtomicBool::new(false));
            let (ready, started) = mpsc::channel();
            let thread_flag = flag.clone();
            let thread = std::thread::Builder::new()
                .name("laqta-highlight".into())
                .spawn(move || run(thread_flag, ready))
                .map_err(|e| PortError::Failed(format!("highlight thread: {e}")))?;
            match started.recv_timeout(START_TIMEOUT) {
                Ok(Ok(())) => Ok(Self { flag, thread }),
                Ok(Err(e)) => {
                    let _ = thread.join();
                    Err(e)
                }
                Err(_) => {
                    // The thread is stuck or gone; it will see the flag if it ever gets going again.
                    flag.store(true, Ordering::Release);
                    Err(PortError::Failed("the highlight thread did not start".into()))
                }
            }
        }

        fn stop(self) {
            self.flag.store(true, Ordering::Release);
            let _ = self.thread.join();
        }
    }

    /// The thread body: opens the popup, reports the result, then follows the pointer until the flag
    /// is set. The popup is destroyed when this function returns.
    fn run(flag: Arc<AtomicBool>, ready: mpsc::Sender<Result<(), PortError>>) {
        let mut overlay = match Overlay::open() {
            Ok(overlay) => overlay,
            Err(e) => {
                let _ = ready.send(Err(e));
                return;
            }
        };
        let _ = ready.send(Ok(()));
        while !flag.load(Ordering::Acquire) {
            if let Err(e) = overlay.step() {
                tracing::warn!("mouse highlighter stopped: {e}");
                break;
            }
            std::thread::sleep(TICK);
        }
    }

    /// The popup and its pixels. Fields drop in order: the window is destroyed before the DC and DIB.
    struct Overlay {
        window: Window,
        // Held for its drop only.
        _surface: Surface,
    }

    impl Overlay {
        fn open() -> Result<Self, PortError> {
            let surface = Surface::new()?;
            let window = Window::create(window_origin(pointer()?, DIAMETER as i32))?;
            let size = SIZE { cx: DIAMETER as i32, cy: DIAMETER as i32 };
            let source = POINT::default();
            let blend =
                BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: u8::MAX, AlphaFormat: AC_SRC_ALPHA as u8 };
            // SAFETY: the window and the memory DC are live and made on this thread. The pointers name
            // locals that outlive the call. The position is left where `Window::create` put it.
            unsafe {
                UpdateLayeredWindow(
                    window.0,
                    None,
                    None,
                    Some(&size as *const SIZE),
                    Some(surface.dc),
                    Some(&source as *const POINT),
                    COLORREF(0),
                    Some(&blend as *const BLENDFUNCTION),
                    ULW_ALPHA,
                )
            }
            .map_err(|e| PortError::Failed(format!("UpdateLayeredWindow: {e}")))?;
            // SAFETY: shows a window made on this thread. Showing without activation leaves focus alone.
            let _ = unsafe { ShowWindow(window.0, SW_SHOWNOACTIVATE) };
            Ok(Self { window, _surface: surface })
        }

        /// Moves the popup to the pointer, keeps it top-most, and dispatches the queued messages.
        fn step(&mut self) -> Result<(), PortError> {
            let (x, y) = window_origin(pointer()?, DIAMETER as i32);
            // SAFETY: the window is live and was made on this thread. No pointers are passed.
            unsafe { SetWindowPos(self.window.0, Some(HWND_TOPMOST), x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE) }
                .map_err(|e| PortError::Failed(format!("SetWindowPos: {e}")))?;
            pump();
            Ok(())
        }
    }

    /// The layered popup, destroyed on drop.
    struct Window(HWND);

    impl Window {
        fn create(origin: (i32, i32)) -> Result<Self, PortError> {
            let (x, y) = origin;
            let class = w!("LaqtaHighlighter");
            // SAFETY: class registration and window creation, both on this thread. The class name and
            // procedure are static. A second registration fails with ERROR_CLASS_ALREADY_EXISTS, which
            // is expected.
            unsafe {
                let instance = GetModuleHandleW(None).ok().map(|m| HINSTANCE(m.0));
                let class_info = WNDCLASSW {
                    lpfnWndProc: Some(overlay_proc),
                    hInstance: instance.unwrap_or_default(),
                    lpszClassName: class,
                    ..Default::default()
                };
                RegisterClassW(&class_info);
                let ex_style = WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
                let hwnd = CreateWindowExW(
                    ex_style,
                    class,
                    w!("Laqta highlighter"),
                    WS_POPUP,
                    x,
                    y,
                    DIAMETER as i32,
                    DIAMETER as i32,
                    None,
                    None,
                    instance,
                    None,
                )
                .map_err(|e| PortError::Failed(format!("CreateWindowExW: {e}")))?;
                Ok(Self(hwnd))
            }
        }
    }

    impl Drop for Window {
        fn drop(&mut self) {
            // SAFETY: the window was made on this thread and is destroyed once, here.
            let _ = unsafe { DestroyWindow(self.0) };
        }
    }

    /// The window procedure. Default handling takes every message.
    unsafe extern "system" fn overlay_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        // SAFETY: default processing takes the message as delivered.
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    /// A memory DC with the disc's DIB selected in, released on drop.
    struct Surface {
        dc: HDC,
        bitmap: HBITMAP,
    }

    impl Surface {
        fn new() -> Result<Self, PortError> {
            let pixels = disc_pixels(DIAMETER);
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: DIAMETER as i32,
                    biHeight: -(DIAMETER as i32), // top-down rows, like the frames
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            // SAFETY: the DC is deleted on every path that does not return it. While the DIB is alive,
            // `bits` points at `pixels.len()` writable 32-bit pixels, which the copy fills.
            unsafe {
                let dc = CreateCompatibleDC(None);
                let bitmap = match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
                    Ok(bitmap) => bitmap,
                    Err(e) => {
                        let _ = DeleteDC(dc);
                        return Err(PortError::Failed(format!("CreateDIBSection: {e}")));
                    }
                };
                std::ptr::copy_nonoverlapping(pixels.as_ptr().cast::<u8>(), bits.cast::<u8>(), pixels.len() * size_of::<u32>());
                SelectObject(dc, bitmap.into());
                Ok(Self { dc, bitmap })
            }
        }
    }

    impl Drop for Surface {
        fn drop(&mut self) {
            // SAFETY: the DC goes first, so the bitmap is no longer selected when it is deleted.
            unsafe {
                let _ = DeleteDC(self.dc);
                let _ = DeleteObject(self.bitmap.into());
            }
        }
    }

    /// The pointer's position in physical desktop pixels.
    fn pointer() -> Result<(i32, i32), PortError> {
        let mut point = POINT::default();
        // SAFETY: `point` is a live out-parameter.
        unsafe { GetCursorPos(&mut point) }.map_err(|e| PortError::Failed(format!("GetCursorPos: {e}")))?;
        Ok((point.x, point.y))
    }

    /// Dispatches the messages queued for this thread, so the queue never grows.
    fn pump() {
        let mut msg = MSG::default();
        // SAFETY: `msg` is a live out-parameter. Only this thread's messages are read.
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DIAMETER, FILL_ALPHA, disc_pixels, premultiplied, window_origin};

    #[test]
    fn premultiplied_scales_the_colour_by_alpha() {
        assert_eq!(premultiplied(255, (255, 255, 0)), 0xffff_ff00);
        assert_eq!(premultiplied(FILL_ALPHA, (255, 255, 0)), 0x5959_5900);
        assert_eq!(premultiplied(0, (255, 165, 0)), 0);
    }

    #[test]
    fn the_disc_has_clear_corners_a_fill_centre_and_a_ring_edge() {
        let size = DIAMETER;
        let pixels = disc_pixels(size);
        assert_eq!(pixels.len(), (size * size) as usize);
        assert_eq!(pixels[0], 0);
        assert_eq!(pixels[(24 * size + 24) as usize], 0x5959_5900);
        assert_eq!(pixels[(24 * size) as usize], 0xffff_a500);
    }

    #[test]
    fn the_disc_covers_about_pi_r_squared_pixels() {
        let covered = disc_pixels(DIAMETER).iter().filter(|pixel| **pixel != 0).count();
        assert!((1780..=1830).contains(&covered), "{covered}");
    }

    #[test]
    fn the_popup_is_centred_on_the_pointer() {
        assert_eq!(window_origin((100, 50), 48), (76, 26));
        assert_eq!(window_origin((0, 0), 48), (-24, -24));
    }
}
