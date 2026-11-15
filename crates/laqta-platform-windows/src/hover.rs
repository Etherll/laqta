//! Windows hover targets: the top-level windows a region selection can snap to, front to
//! back, plus the UI elements of the focused window and the focused window itself.

use std::mem::size_of;
use std::sync::atomic::{AtomicBool, Ordering};

use laqta_domain::PhysRect;
use laqta_domain::region::{HoverTarget, TargetKind};
use laqta_ports::{BoxFuture, HoverSource, PortError};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT, TRUE};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation, TreeScope_Descendants};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GWL_EXSTYLE, GetClassNameW, GetForegroundWindow, GetWindowLongPtrW, GetWindowRect, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindowVisible, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
};
use windows::core::{BOOL, PWSTR};

use crate::window_rules::{exe_name, is_snap_candidate, window_rect};

/// The most UI elements of the focused window that are considered. `FindAll` returns the whole
/// subtree, so this bounds the cross-process calls made per hover refresh.
const MAX_ELEMENTS: i32 = 400;

/// Whether Laqta's own windows are snap targets, set through `HoverSource::set_include_own_windows`.
/// A process-wide flag, so `WindowsHover` stays a unit struct.
static INCLUDE_OWN_WINDOWS: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Default)]
pub struct WindowsHover;

impl HoverSource for WindowsHover {
    fn targets(&self) -> BoxFuture<'_, Result<Vec<HoverTarget>, PortError>> {
        Box::pin(blocking::unblock(top_level_targets))
    }

    fn active_window(&self) -> BoxFuture<'_, Result<Option<HoverTarget>, PortError>> {
        Box::pin(blocking::unblock(foreground_target))
    }

    fn set_include_own_windows(&self, include: bool) {
        INCLUDE_OWN_WINDOWS.store(include, Ordering::Relaxed);
    }
}

/// Every snap candidate, front to back. `EnumWindows` walks the z-order from the top. The focused
/// window's UI elements go in just before it, since they are in front of it.
fn top_level_targets() -> Result<Vec<HoverTarget>, PortError> {
    // Declared first, so every UI Automation object is released before COM is uninitialised.
    let com = ComApartment::enter();
    let mut found: Vec<HoverTarget> = Vec::new();
    // SAFETY: the callback runs only during this call, while `found` is borrowed.
    unsafe { EnumWindows(Some(collect), LPARAM((&raw mut found) as isize)) }.map_err(|e| PortError::Failed(format!("EnumWindows: {e}")))?;
    // SAFETY: takes no arguments; the result may be null, which gives id 0 and matches no target.
    let foreground = hwnd_id(unsafe { GetForegroundWindow() });
    if let Some(index) = found.iter().position(|t| t.handle == foreground).filter(|_| com.is_some())
        && let Some(window) = hwnd_from_id(found[index].handle)
    {
        let elements = ui_elements(window, &found[index].process);
        found.splice(index..index, elements);
    }
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
/// the region overlay never snaps to itself, unless captures include Laqta's own windows. Even then
/// its tool and popup windows (the overlay, toasts, the recording bar) are skipped.
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
    if pid == std::process::id() && (tool || !INCLUDE_OWN_WINDOWS.load(Ordering::Relaxed)) {
        return None;
    }
    let rect = frame_rect(hwnd)?;
    let title = window_text(hwnd);
    let class = class_name(hwnd);
    if title.is_empty() && class.is_empty() {
        return None;
    }
    Some(HoverTarget { handle: hwnd_id(hwnd), rect, title, process: process_name(pid), kind: TargetKind::Window })
}

/// The `u64` a `HoverTarget::handle` carries for `hwnd`.
pub(crate) fn hwnd_id(hwnd: HWND) -> u64 {
    hwnd.0.addr() as u64
}

/// The window a `HoverTarget::handle` names. `None` for 0.
pub(crate) fn hwnd_from_id(id: u64) -> Option<HWND> {
    let addr = usize::try_from(id).ok().filter(|&a| a != 0)?;
    Some(HWND(std::ptr::without_provenance_mut(addr)))
}

/// True when DWM reports the window as cloaked. A failed query counts as not cloaked.
fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    // SAFETY: `cloaked` is a u32, and the size passed is its size.
    let queried = unsafe { DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, (&raw mut cloaked).cast(), size_of::<u32>() as u32) };
    queried.is_ok() && cloaked != 0
}

/// The window's visible frame in physical pixels, falling back to its outer rect when DWM has none.
pub(crate) fn frame_rect(hwnd: HWND) -> Option<PhysRect> {
    let mut rect = RECT::default();
    // SAFETY: `rect` is a RECT, and the size passed is its size.
    let framed = unsafe { DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, (&raw mut rect).cast(), size_of::<RECT>() as u32) };
    if framed.is_err() {
        // SAFETY: `rect` is a valid out-parameter.
        unsafe { GetWindowRect(hwnd, &raw mut rect) }.ok()?;
    }
    window_rect(rect.left, rect.top, rect.right, rect.bottom)
}

/// Initialises COM on this thread for UI Automation, and uninitialises it on drop when this call
/// started it. `None` when COM is already set up in another mode (`RPC_E_CHANGED_MODE`), so the
/// caller skips the elements and keeps the windows.
struct ComApartment;

impl ComApartment {
    fn enter() -> Option<Self> {
        // SAFETY: plain initialisation with no reserved pointer.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        // S_OK and S_FALSE both take a reference that `Drop` releases.
        hr.is_ok().then_some(ComApartment)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        // SAFETY: balances the successful CoInitializeEx in `enter`.
        unsafe { CoUninitialize() };
    }
}

/// The UI elements inside `window`, for hover snapping, with its `process` name. The first
/// `MAX_ELEMENTS` of the subtree are considered; offscreen and empty ones are skipped. Failure of
/// any UI Automation call leaves the list short, never an error.
fn ui_elements(window: HWND, process: &str) -> Vec<HoverTarget> {
    let Ok(automation) = create_automation() else {
        return Vec::new();
    };
    walk_elements(&automation, window, process).unwrap_or_default()
}

fn create_automation() -> windows::core::Result<IUIAutomation> {
    // SAFETY: creates the UI Automation client object. COM is initialised on this thread (see `ComApartment`).
    unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
}

fn walk_elements(automation: &IUIAutomation, window: HWND, process: &str) -> windows::core::Result<Vec<HoverTarget>> {
    // SAFETY: COM calls on live UI Automation objects; each returned element is dropped at the end of its loop turn.
    unsafe {
        let root = automation.ElementFromHandle(window)?;
        let condition = automation.CreateTrueCondition()?;
        let all = root.FindAll(TreeScope_Descendants, &condition)?;
        let count = all.Length()?.clamp(0, MAX_ELEMENTS);
        let mut elements = Vec::new();
        for index in 0..count {
            let Ok(element) = all.GetElement(index) else {
                continue;
            };
            if element.CurrentIsOffscreen().is_ok_and(|offscreen| offscreen.as_bool()) {
                continue;
            }
            let Ok(bounds) = element.CurrentBoundingRectangle() else {
                continue;
            };
            let Some(rect) = window_rect(bounds.left, bounds.top, bounds.right, bounds.bottom) else {
                continue;
            };
            let title = element.CurrentName().map(|name| name.to_string()).unwrap_or_default();
            elements.push(HoverTarget { handle: hwnd_id(window), rect, title, process: process.to_owned(), kind: TargetKind::Element });
        }
        // The subtree comes in pre-order, parents before children. The overlay takes the first target
        // under the pointer, so reversing puts the innermost element first.
        elements.reverse();
        Ok(elements)
    }
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
