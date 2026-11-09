//! Draws the mouse cursor onto captured frames. For each frame the cursor overlaps, the
//! frame's patch under the cursor is copied into a DIB, GDI draws the cursor over it, and the patch
//! is written back. Drawing over the real pixels handles alpha and XOR cursors alike.

use std::ffi::c_void;
use std::mem::size_of;
use std::ptr::null_mut;

use laqta_domain::{Bitmap, Frame, PhysRect, PixelFormat};
use windows::Win32::Foundation::E_FAIL;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetObjectW,
    SelectObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CURSOR_SHOWING, CURSORINFO, DI_NORMAL, DrawIconEx, GetCursorInfo, GetIconInfo, HICON, ICONINFO,
};
use windows::core::Result;

use crate::patch::{extract, overlap, write_back};

/// The largest cursor edge drawn. Anything bigger is skipped rather than allocated.
const MAX_EDGE: i32 = 512;

/// Draws the cursor, when one is showing, onto every frame it overlaps. On failure the frames are
/// left as they were, and the reason goes to the debug log.
pub(crate) fn draw_onto(frames: &mut [Frame]) {
    if let Err(e) = draw(frames.iter_mut().map(|f| (&mut f.image, f.monitor.rect))) {
        tracing::debug!("cursor not drawn: {e}");
    }
}

/// Draws the cursor, when one is showing, onto `image`, whose top-left pixel is at `origin` on the
/// virtual desktop. Failure is logged and leaves the image as it was.
pub(crate) fn draw_onto_image(image: &mut Bitmap, origin: PhysRect) {
    if let Err(e) = draw(std::iter::once((image, origin))) {
        tracing::debug!("cursor not drawn: {e}");
    }
}

/// Draws the cursor onto each `(image, origin)` pair that it overlaps.
fn draw<'a>(images: impl IntoIterator<Item = (&'a mut Bitmap, PhysRect)>) -> Result<()> {
    let mut info = CURSORINFO { cbSize: size_of::<CURSORINFO>() as u32, ..Default::default() };
    // SAFETY: `info` is sized as GetCursorInfo requires.
    unsafe { GetCursorInfo(&raw mut info) }?;
    if info.flags.0 & CURSOR_SHOWING.0 == 0 {
        return Ok(());
    }
    let icon = HICON(info.hCursor.0);
    let mut icon_info = ICONINFO::default();
    // SAFETY: `icon` is the cursor the system reports; the bitmaps GetIconInfo returns are owned by
    // `_bitmaps`, which deletes them on every path out of this function.
    unsafe { GetIconInfo(icon, &raw mut icon_info) }?;
    let _bitmaps = IconBitmaps(icon_info);
    let Some((width, height)) = cursor_size(&icon_info) else {
        return Ok(());
    };
    let cursor = PhysRect {
        x: info.ptScreenPos.x.saturating_sub(icon_info.xHotspot as i32),
        y: info.ptScreenPos.y.saturating_sub(icon_info.yHotspot as i32),
        width: width as u32,
        height: height as u32,
    };
    for (image, origin) in images {
        draw_on_image(image, origin, icon, cursor)?;
    }
    Ok(())
}

/// The bitmaps `GetIconInfo` returns. The caller owns them, so they are deleted when this drops.
struct IconBitmaps(ICONINFO);

impl Drop for IconBitmaps {
    fn drop(&mut self) {
        for bitmap in [self.0.hbmMask, self.0.hbmColor] {
            if !bitmap.is_invalid() {
                // SAFETY: GetIconInfo created this bitmap for us, and nothing else deletes it.
                let _ = unsafe { DeleteObject(bitmap.into()) };
            }
        }
    }
}

/// The cursor's pixel size. A colour cursor's size is its colour bitmap. A monochrome cursor has
/// no colour bitmap, and its mask stacks the AND bitmap over the XOR bitmap, so half its height.
fn cursor_size(info: &ICONINFO) -> Option<(i32, i32)> {
    let (bitmap, stacked) = if info.hbmColor.is_invalid() { (info.hbmMask, 2) } else { (info.hbmColor, 1) };
    let mut bm = BITMAP::default();
    // SAFETY: `bm` is a BITMAP, and the size passed is its size.
    let copied = unsafe { GetObjectW(bitmap.into(), size_of::<BITMAP>() as i32, Some((&raw mut bm).cast())) };
    if copied == 0 {
        return None;
    }
    let (width, height) = (bm.bmWidth, bm.bmHeight / stacked);
    ((1..=MAX_EDGE).contains(&width) && (1..=MAX_EDGE).contains(&height)).then_some((width, height))
}

/// Draws the cursor onto `image`, whose top-left pixel is at `origin`, if the cursor's rect overlaps it.
fn draw_on_image(image: &mut Bitmap, origin: PhysRect, icon: HICON, cursor: PhysRect) -> Result<()> {
    if image.format() != PixelFormat::Bgra8 {
        return Ok(());
    }
    let (width, height) = (image.width(), image.height());
    let local = PhysRect { x: cursor.x.saturating_sub(origin.x), y: cursor.y.saturating_sub(origin.y), ..cursor };
    if overlap(local, PhysRect { x: 0, y: 0, width, height }).is_none() {
        return Ok(());
    }
    let mut patch = extract(image.pixels(), width, height, local);
    render_into(icon, local.width as i32, local.height as i32, &mut patch)?;
    let mut pixels = image.pixels().to_vec();
    write_back(&mut pixels, width, height, local, &patch);
    if let Ok(drawn) = Bitmap::new(width, height, PixelFormat::Bgra8, pixels.into()) {
        *image = drawn;
    }
    Ok(())
}

/// Draws `icon` at `width` x `height` over `pixels`, a BGRA patch of that size, through a GDI DIB.
fn render_into(icon: HICON, width: i32, height: i32, pixels: &mut [u8]) -> Result<()> {
    let len = width as usize * height as usize * 4;
    if pixels.len() != len {
        return Err(E_FAIL.into());
    }
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height, // top-down rows, like the frames
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut c_void = null_mut();
    // SAFETY: the memory DC, the DIB and the selection are all released on every path below. While
    // the DIB is selected and alive, `bits` points at `len` writable bytes.
    unsafe {
        let mem = CreateCompatibleDC(None);
        let dib = match CreateDIBSection(Some(mem), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(dib) => dib,
            Err(e) => {
                let _ = DeleteDC(mem);
                return Err(e);
            }
        };
        let old = SelectObject(mem, dib.into());
        let dib_pixels = std::slice::from_raw_parts_mut(bits.cast::<u8>(), len);
        dib_pixels.copy_from_slice(pixels);
        let drawn = DrawIconEx(mem, 0, 0, icon, width, height, 0, None, DI_NORMAL);
        if drawn.is_ok() {
            pixels.copy_from_slice(dib_pixels);
        }
        SelectObject(mem, old);
        let _ = DeleteObject(dib.into());
        let _ = DeleteDC(mem);
        drawn
    }
}
