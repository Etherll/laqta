//! Windows hover targets (M1 §4.5): the top-level windows a region selection can snap to, front to
//! back, plus the focused window.

use std::mem::size_of;

use laqta_domain::PhysRect;
use laqta_domain::region::{HoverTarget, TargetKind};
use laqta_ports::{BoxFuture, HoverSource, PortError};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT, TRUE};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GWL_EXSTYLE, GetClassNameW, GetForegroundWindow, GetWindowLongPtrW, GetWindowRect, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindowVisible, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
};
use windows::core::{BOOL, PWSTR};

use crate::window_rules::{exe_name, is_snap_candidate, window_rect};

#[derive(Debug, Default)]
pub struct WindowsHover;

impl HoverSource for WindowsHover {
    fn targets(&self) -> BoxFuture<'_, Result<Vec<HoverTarget>, PortError>> {
        Box::pin(blocking::unblock(top_level_targets))
    }

    fn active_window(&self) -> BoxFuture<'_, Result<Option<HoverTarget>, PortError>> {
        Box::pin(blocking::unblock(foreground_target))
    }
}

/// Every snap candidate, front to back (`EnumWindows` walks the z-order from the top).
fn top_level_targets() -> Result<Vec<HoverTarget>, PortError> {
    let mut found: Vec<HoverTarget> = Vec::new();
    // SAFETY: the callback runs only during this call, while `found` is borrowed.
    unsafe { EnumWindows(Some(collect), LPARAM((&raw mut found) as isize)) }.map_err(|e| PortError::Failed(format!("EnumWindows: {e}")))?;
    Ok(found)
}

/// `EnumWindows` callback. `data` is the `&mut Vec<HoverTarget>` that `top_level_targets` passed in.
unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> BOOL {
    // SAFETY: `data` points at the vector borrowed for the whole enumeration.
    let found = unsafe { &mut *(data.0 as *mut Vec<HoverTarget>) };
    if let Some(target) = describe(hwnd) {
        found.push(target);
    }
    TRUE
}

fn foreground_target() -> Result<Option<HoverTarget>, PortError> {
    // SAFETY: takes no arguments; the result may be null.
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return Ok(None);
    }
    Ok(describe(hwnd))
}

/// The hover target for `hwnd`, if it is a snap candidate. Windows of this process are skipped, so
/// the region overlay never snaps to itself.
fn describe(hwnd: HWND) -> Option<HoverTarget> {
    // SAFETY: read-only queries on a window handle; an invalid handle yields false or zero.
    let (visible, iconic, style) =
        unsafe { (IsWindowVisible(hwnd).as_bool(), IsIconic(hwnd).as_bool(), GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32) };
    let tool = style & WS_EX_TOOLWINDOW.0 != 0;
    let app = style & WS_EX_APPWINDOW.0 != 0;
    if !is_snap_candidate(visible, iconic, is_cloaked(hwnd), tool, app) {
        return None;
    }
    let mut pid = 0u32;
    // SAFETY: `pid` is a valid out-parameter.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&raw mut pid)) };
    if pid == std::process::id() {
        return None;
    }
    let rect = frame_rect(hwnd)?;
    let title = window_text(hwnd);
    let class = class_name(hwnd);
    if title.is_empty() && class.is_empty() {
        return None;
    }
    Some(HoverTarget { handle: 0, rect, title, process: process_name(pid), kind: TargetKind::Window })
}

/// True when DWM reports the window as cloaked. A failed query counts as not cloaked.
fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    // SAFETY: `cloaked` is a u32, and the size passed is its size.
    let queried = unsafe { DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, (&raw mut cloaked).cast(), size_of::<u32>() as u32) };
    queried.is_ok() && cloaked != 0
}

/// The window's visible frame in physical pixels, falling back to its outer rect when DWM has none.
fn frame_rect(hwnd: HWND) -> Option<PhysRect> {
    let mut rect = RECT::default();
    // SAFETY: `rect` is a RECT, and the size passed is its size.
    let framed = unsafe { DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, (&raw mut rect).cast(), size_of::<RECT>() as u32) };
    if framed.is_err() {
        // SAFETY: `rect` is a valid out-parameter.
        unsafe { GetWindowRect(hwnd, &raw mut rect) }.ok()?;
    }
    window_rect(rect.left, rect.top, rect.right, rect.bottom)
}

fn window_text(hwnd: HWND) -> String {
    let mut buf = [0u16; 512];
    // SAFETY: the buffer is valid for `buf.len()` units.
    let copied = unsafe { GetWindowTextW(hwnd, &mut buf) };
    let len = usize::try_from(copied).unwrap_or(0).min(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    // SAFETY: the buffer is valid for `buf.len()` units.
    let copied = unsafe { GetClassNameW(hwnd, &mut buf) };
    let len = usize::try_from(copied).unwrap_or(0).min(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// The executable's file name for process `pid`, or empty when it cannot be read (for example a
/// protected process).
fn process_name(pid: u32) -> String {
    // SAFETY: a limited-rights handle, closed below on every path.
    let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else {
        return String::new();
    };
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` holds `len` units; on success `len` is the number of units written.
    let queried = unsafe { QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &raw mut len) };
    // SAFETY: `process` is a handle from OpenProcess, closed once.
    let _ = unsafe { CloseHandle(process) };
    match queried {
        Ok(()) => {
            let len = (len as usize).min(buf.len());
            exe_name(&String::from_utf16_lossy(&buf[..len]))
        }
        Err(_) => String::new(),
    }
}
