//! Window tools on Windows (M5 D7): borderless by clearing the caption and sizing-frame styles,
//! top-most through `SetWindowPos`, and the inspector's properties. Handles come from hover targets.

use laqta_domain::region::HoverTarget;
use laqta_ports::{PortError, WindowOps};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GWL_STYLE, GetClassNameW, GetWindowLongPtrW, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, HWND_NOTOPMOST,
    HWND_TOPMOST, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos, WS_CAPTION,
    WS_THICKFRAME,
};

use crate::hover::hwnd_from_id;

/// The styles borderless clears and restores.
const FRAME_STYLES: isize = (WS_CAPTION.0 | WS_THICKFRAME.0) as isize;

/// Window tools for hover targets.
#[derive(Debug)]
pub struct WindowsWindowOps;

impl WindowOps for WindowsWindowOps {
    fn set_borderless(&self, window: &HoverTarget, borderless: bool) -> Result<(), PortError> {
        let hwnd = window_handle(window)?;
        // SAFETY: reads the style word of a window handle. An invalid or closed window gives 0.
        let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) };
        if style == 0 {
            return Err(gone());
        }
        let next = if borderless { style & !FRAME_STYLES } else { style | FRAME_STYLES };
        // SAFETY: the same handle, read just above. The style word is plain data.
        unsafe { SetWindowLongPtrW(hwnd, GWL_STYLE, next) };
        // SAFETY: the same handle. Without a move, size or z-order change, the flags only ask
        // Windows to recompute the frame for the new style.
        unsafe { SetWindowPos(hwnd, None, 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE) }
            .map_err(|e| PortError::Failed(format!("SetWindowPos: {e}")))
    }

    fn set_topmost(&self, window: &HoverTarget, topmost: bool) -> Result<(), PortError> {
        let hwnd = window_handle(window)?;
        let after = if topmost { HWND_TOPMOST } else { HWND_NOTOPMOST };
        // SAFETY: the same handle. Only the z-order changes; the position and size stay.
        unsafe { SetWindowPos(hwnd, Some(after), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) }
            .map_err(|e| PortError::Failed(format!("SetWindowPos: {e}")))
    }

    fn inspect(&self, window: &HoverTarget) -> Result<Vec<(String, String)>, PortError> {
        let hwnd = window_handle(window)?;
        let mut pid = 0u32;
        // SAFETY: `pid` is a valid out-parameter.
        unsafe { GetWindowThreadProcessId(hwnd, Some(&raw mut pid)) };
        // SAFETY: read-only queries on the handle. An invalid handle gives 0.
        let (style, exstyle) = unsafe { (GetWindowLongPtrW(hwnd, GWL_STYLE), GetWindowLongPtrW(hwnd, GWL_EXSTYLE)) };
        if style == 0 {
            return Err(gone());
        }
        let mut rect = RECT::default();
        // SAFETY: `rect` is a valid RECT.
        unsafe { GetWindowRect(hwnd, &raw mut rect) }.map_err(|e| PortError::Failed(format!("GetWindowRect: {e}")))?;
        // SAFETY: read-only query on the handle; 0 when the handle is invalid.
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        Ok(vec![
            ("class".to_owned(), class_name(hwnd)),
            ("title".to_owned(), window_text(hwnd)),
            ("pid".to_owned(), pid.to_string()),
            ("style".to_owned(), format!("{:#010x}", style as u32)),
            ("exstyle".to_owned(), format!("{:#010x}", exstyle as u32)),
            ("rect".to_owned(), format!("{},{},{},{}", rect.left, rect.top, rect.right, rect.bottom)),
            ("dpi".to_owned(), dpi.to_string()),
        ])
    }
}

fn window_handle(window: &HoverTarget) -> Result<HWND, PortError> {
    hwnd_from_id(window.handle).ok_or_else(|| PortError::Failed("the target has no window handle".into()))
}

fn gone() -> PortError {
    PortError::Failed("the window is gone".into())
}

fn class_name(hwnd: HWND) -> String {
    let mut buffer = [0u16; 256];
    // SAFETY: `buffer` is writable for its whole length.
    let len = unsafe { GetClassNameW(hwnd, &mut buffer) };
    let len = usize::try_from(len).unwrap_or(0).min(buffer.len());
    String::from_utf16_lossy(&buffer[..len])
}

fn window_text(hwnd: HWND) -> String {
    let mut buffer = [0u16; 1024];
    // SAFETY: `buffer` is writable for its whole length.
    let len = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    let len = usize::try_from(len).unwrap_or(0).min(buffer.len());
    String::from_utf16_lossy(&buffer[..len])
}
