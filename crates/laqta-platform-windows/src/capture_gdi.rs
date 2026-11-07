//! GDI `BitBlt` capture, one monitor at a time. Used when Desktop Duplication is unavailable
//! (no interactive session, RDP, access lost).

use std::mem::size_of;

use laqta_domain::{Bitmap, Frame, Monitor, MonitorId, PhysRect, PixelFormat};
use laqta_ports::PortError;
use windows::Win32::Foundation::{LPARAM, RECT, TRUE};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject,
    EnumDisplayMonitors, GetDC, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW, ReleaseDC, SRCCOPY, SelectObject,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::core::BOOL;

/// Describes a monitor in physical pixels (the process is Per-Monitor-V2 aware).
pub(crate) fn monitor_from_handle(handle: HMONITOR) -> Option<Monitor> {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    // SAFETY: `info` is a valid MONITORINFOEXW with cbSize set, as GetMonitorInfoW requires.
    let ok = unsafe { GetMonitorInfoW(handle, (&raw mut info).cast::<MONITORINFO>()) };
    if !ok.as_bool() {
        return None;
    }
    let r = info.monitorInfo.rcMonitor;
    let len = info.szDevice.iter().position(|&c| c == 0).unwrap_or(info.szDevice.len());
    let device = String::from_utf16_lossy(&info.szDevice[..len]);
    let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
    // SAFETY: out-pointers to locals.
    let scale = match unsafe { GetDpiForMonitor(handle, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) } {
        Ok(()) => dpi_x as f32 / 96.0,
        Err(_) => 1.0,
    };
    Some(Monitor {
        id: MonitorId(device.clone()),
        name: device,
        rect: PhysRect { x: r.left, y: r.top, width: r.right.abs_diff(r.left), height: r.bottom.abs_diff(r.top) },
        scale,
        primary: info.monitorInfo.dwFlags & 1 != 0, // MONITORINFOF_PRIMARY
    })
}

fn monitor_handles() -> Vec<HMONITOR> {
    unsafe extern "system" fn push(handle: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the `&mut Vec<HMONITOR>` passed below, alive for the whole call.
        let list = unsafe { &mut *(data.0 as *mut Vec<HMONITOR>) };
        list.push(handle);
        TRUE
    }
    let mut handles: Vec<HMONITOR> = Vec::new();
    // SAFETY: the callback only runs during this call, while `handles` is borrowed.
    let _ = unsafe { EnumDisplayMonitors(None, None, Some(push), LPARAM((&raw mut handles) as isize)) };
    handles
}

pub(crate) fn capture() -> Result<Vec<Frame>, PortError> {
    let frames: Vec<Frame> = monitor_handles()
        .into_iter()
        .filter_map(monitor_from_handle)
        .map(|monitor| capture_rect(monitor.rect).map(|image| Frame { monitor, image }))
        .collect::<Result<_, _>>()?;
    if frames.is_empty() {
        return Err(PortError::Failed("no monitors found".into()));
    }
    Ok(frames)
}

fn capture_rect(rect: PhysRect) -> Result<Bitmap, PortError> {
    let (w, h) = (rect.width as i32, rect.height as i32);
    let fail = |what: &str| PortError::Failed(format!("GDI capture: {what}"));
    // SAFETY: standard GDI sequence; every handle created here is released on every path.
    unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            return Err(fail("GetDC"));
        }
        let mem = CreateCompatibleDC(Some(screen));
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // top-down rows
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let result = match CreateDIBSection(Some(mem), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(dib) => {
                let old = SelectObject(mem, dib.into());
                let blit = BitBlt(mem, 0, 0, w, h, Some(screen), rect.x, rect.y, SRCCOPY | CAPTUREBLT);
                let out = blit.map_err(|e| fail(&e.to_string())).and_then(|()| {
                    let len = rect.width as usize * rect.height as usize * 4;
                    let mut px = std::slice::from_raw_parts(bits as *const u8, len).to_vec();
                    for p in px.as_chunks_mut::<4>().0 {
                        p[3] = 255; // BitBlt leaves alpha at 0
                    }
                    Bitmap::new(rect.width, rect.height, PixelFormat::Bgra8, px.into()).map_err(|e| fail(&e.to_string()))
                });
                SelectObject(mem, old);
                let _ = DeleteObject(dib.into());
                out
            }
            Err(e) => Err(fail(&e.to_string())),
        };
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        result
    }
}
