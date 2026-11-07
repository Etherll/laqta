//! Transparent window capture (ShareX's Screenshot_Transparent). The window is captured twice, once
//! over a white backdrop and once over black, and the alpha comes from the difference (see
//! `backdrop`). Each backdrop is a popup placed directly behind the window in z-order, so the
//! window's own pixels, including rounded corners and translucent parts, composite over it.

use laqta_domain::region::{HoverTarget, TargetKind};
use laqta_domain::{Bitmap, PhysRect, PixelFormat};
use laqta_ports::PortError;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::{BLACK_BRUSH, GetStockObject, HBRUSH, UpdateWindow, WHITE_BRUSH};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, SWP_NOACTIVATE, SWP_SHOWWINDOW, SetWindowPos, WNDCLASSW,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::w;

use crate::backdrop::alpha_from_backdrops;
use crate::{capture_gdi, cursor, hover};

/// Captures `target`, a window, keeping its transparency. The result is straight-alpha BGRA, sized
/// to the window's frame as DWM reports it now. The cursor, when asked for, is drawn into both
/// captures, so it is opaque over the window and clear where the window is transparent.
pub(crate) fn capture(target: &HoverTarget, show_cursor: bool) -> Result<Bitmap, PortError> {
    if target.kind != TargetKind::Window {
        return Err(PortError::Unsupported("transparent capture of a UI element".into()));
    }
    let hwnd = hover::hwnd_from_id(target.handle).ok_or_else(|| PortError::Failed("window handle is missing".into()))?;
    let rect = hover::frame_rect(hwnd).ok_or_else(|| PortError::Failed("window frame is not available".into()))?;
    // The port promises a result the size of `target.rect`. A move is harmless, since only the
    // pixels matter, but a resize since the list was made is an error.
    if (rect.width, rect.height) != (target.rect.width, target.rect.height) {
        return Err(PortError::Failed("window was resized after it was listed".into()));
    }
    let white = over_backdrop(hwnd, rect, Backdrop::White, show_cursor)?;
    let black = over_backdrop(hwnd, rect, Backdrop::Black, show_cursor)?;
    let pixels = alpha_from_backdrops(white.pixels(), black.pixels());
    Bitmap::new(rect.width, rect.height, PixelFormat::Bgra8, pixels.into())
        .map_err(|e| PortError::Failed(format!("transparent capture: {e}")))
}

#[derive(Clone, Copy)]
enum Backdrop {
    White,
    Black,
}

/// Captures `rect` with a `backdrop` window directly behind `target`. The backdrop is destroyed on
/// every path, including a failed capture.
fn over_backdrop(target: HWND, rect: PhysRect, backdrop: Backdrop, show_cursor: bool) -> Result<Bitmap, PortError> {
    let window = show_backdrop(target, rect, backdrop)?;
    let captured = capture_gdi::capture_rect(rect).map(|mut image| {
        if show_cursor {
            cursor::draw_onto_image(&mut image, rect);
        }
        image
    });
    // SAFETY: `window` was created on this thread by `show_backdrop` and is destroyed here, once.
    let _ = unsafe { DestroyWindow(window) };
    captured
}

/// Creates the backdrop window with `rect`, places it directly below `target` in z-order, and shows
/// it without activating. Returns once DWM has composed a frame that includes it.
fn show_backdrop(target: HWND, rect: PhysRect, backdrop: Backdrop) -> Result<HWND, PortError> {
    let (class, brush) = match backdrop {
        Backdrop::White => (w!("LaqtaBackdropWhite"), WHITE_BRUSH),
        Backdrop::Black => (w!("LaqtaBackdropBlack"), BLACK_BRUSH),
    };
    let (width, height) = (rect.width as i32, rect.height as i32);
    // SAFETY: window creation and placement, all on this thread. Every handle is made here.
    unsafe {
        let instance = GetModuleHandleW(None).ok().map(|m| HINSTANCE(m.0));
        let class_info = WNDCLASSW {
            lpfnWndProc: Some(backdrop_proc),
            hInstance: instance.unwrap_or_default(),
            // The class brush paints the client area when it is first updated.
            hbrBackground: HBRUSH(GetStockObject(brush).0),
            lpszClassName: class,
            ..Default::default()
        };
        // A second registration fails with ERROR_CLASS_ALREADY_EXISTS, which is expected. Any real
        // problem shows up in CreateWindowExW below.
        RegisterClassW(&class_info);
        let window = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class,
            w!(""),
            WS_POPUP,
            rect.x,
            rect.y,
            width,
            height,
            None,
            None,
            instance,
            None,
        )
        .map_err(|e| PortError::Failed(format!("backdrop window: {e}")))?;
        if let Err(e) = SetWindowPos(window, Some(target), rect.x, rect.y, width, height, SWP_NOACTIVATE | SWP_SHOWWINDOW) {
            let _ = DestroyWindow(window);
            return Err(PortError::Failed(format!("backdrop placement: {e}")));
        }
        let _ = UpdateWindow(window);
        // Waits for the next DWM composition, so the screen shows the backdrop before it is read.
        // Fails when composition is off, which is harmless here.
        let _ = DwmFlush();
        Ok(window)
    }
}

/// The backdrop's window procedure. Default handling paints the background from the class brush.
unsafe extern "system" fn backdrop_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: default processing takes the message as delivered.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
