use std::sync::Arc;

use laqta_domain::region::HoverTarget;
use laqta_domain::{Bitmap, Frame, Monitor, MonitorId, PhysRect, PixelFormat};
use laqta_ports::{BoxFuture, CaptureRequest, PortError, ScreenSource};
use x11rb::connection::Connection;
use x11rb::errors::ReplyError;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xfixes::{ConnectionExt as _, GetCursorImageReply};
use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat};
use x11rb::rust_connection::RustConnection;

use super::fail;

/// Captures each RandR monitor from the root window with `GetImage`, and draws the XFixes cursor
/// onto the frames when the request asks for it.
///
/// Plain `GetImage` only; the XShm path is not used.
#[derive(Debug)]
pub struct X11Screen;

impl ScreenSource for X11Screen {
    fn capture_all(&self) -> BoxFuture<'_, Result<Vec<Frame>, PortError>> {
        Box::pin(blocking::unblock(|| capture_frames(false)))
    }

    fn capture(&self, request: CaptureRequest) -> BoxFuture<'_, Result<Vec<Frame>, PortError>> {
        Box::pin(blocking::unblock(move || capture_frames(request.show_cursor)))
    }

    fn capture_window(&self, target: &HoverTarget, request: CaptureRequest) -> BoxFuture<'_, Result<Bitmap, PortError>> {
        let handle = target.handle;
        Box::pin(blocking::unblock(move || capture_window_bitmap(handle, request.show_cursor)))
    }
}

pub(crate) fn capture_frames(show_cursor: bool) -> Result<Vec<Frame>, PortError> {
    let (conn, screen_num) = x11rb::connect(None).map_err(fail)?;
    let setup = conn.setup();
    let screen = &setup.roots[screen_num];
    let root = screen.root;
    let bpp = setup.pixmap_formats.iter().find(|f| f.depth == screen.root_depth).map(|f| f.bits_per_pixel).unwrap_or(0);
    if bpp != 32 {
        return Err(PortError::Unsupported(format!("x11: root depth {} with {bpp} bpp", screen.root_depth)));
    }
    let monitors = match conn.randr_get_monitors(root, true).map_err(fail)?.reply() {
        Ok(r) if !r.monitors.is_empty() => r
            .monitors
            .iter()
            .map(|m| {
                let name = conn
                    .get_atom_name(m.name)
                    .ok()
                    .and_then(|c| c.reply().ok())
                    .map(|r| String::from_utf8_lossy(&r.name).into_owned())
                    .unwrap_or_default();
                Monitor {
                    id: MonitorId(if name.is_empty() { format!("{}", m.name) } else { name.clone() }),
                    name,
                    rect: PhysRect { x: m.x.into(), y: m.y.into(), width: m.width.into(), height: m.height.into() },
                    scale: 1.0,
                    primary: m.primary,
                }
            })
            .collect::<Vec<_>>(),
        _ => vec![Monitor {
            id: MonitorId("screen".into()),
            name: "screen".into(),
            rect: PhysRect { x: 0, y: 0, width: screen.width_in_pixels.into(), height: screen.height_in_pixels.into() },
            scale: 1.0,
            primary: true,
        }],
    };
    // Pixels stay as plain bytes until the cursor is drawn, so no frame is copied out of an `Arc`.
    let mut captured = Vec::with_capacity(monitors.len());
    for monitor in monitors {
        let r = monitor.rect;
        let reply = conn
            .get_image(ImageFormat::Z_PIXMAP, root, r.x as i16, r.y as i16, r.width as u16, r.height as u16, !0)
            .map_err(fail)?
            .reply()
            .map_err(fail)?;
        let mut data = reply.data;
        if data.len() != (r.width * r.height * 4) as usize {
            return Err(fail(format!("unexpected image size {} for {}x{}", data.len(), r.width, r.height)));
        }
        // Little-endian ZPixmap at 32 bpp is BGRX; force opaque alpha.
        for px in data.as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
        captured.push((monitor, data));
    }
    if show_cursor {
        draw_cursor(&conn, &mut captured);
    }
    captured
        .into_iter()
        .map(|(monitor, data)| {
            let r = monitor.rect;
            let image = Bitmap::new(r.width, r.height, PixelFormat::Bgra8, Arc::from(data)).map_err(fail)?;
            Ok(Frame { monitor, image })
        })
        .collect()
}

/// Captures one window's client area with `GetImage` on the window itself, so the frame is the
/// window's geometry and not the frame the window manager draws around it. A 32-bit window's pixels
/// are premultiplied and come out straight; a 24-bit window has no alpha and comes out opaque.
pub(crate) fn capture_window_bitmap(handle: u64, show_cursor: bool) -> Result<Bitmap, PortError> {
    let window =
        u32::try_from(handle).ok().filter(|id| *id != 0).ok_or_else(|| PortError::Unsupported("x11: target has no window id".into()))?;
    let (conn, screen_num) = x11rb::connect(None).map_err(fail)?;
    let setup = conn.setup();
    let root = setup.roots[screen_num].root;
    let geometry = conn.get_geometry(window).map_err(fail)?.reply().map_err(fail)?;
    let (width, height, depth) = (geometry.width, geometry.height, geometry.depth);
    let has_alpha = match depth {
        32 => true,
        24 => false,
        _ => return Err(PortError::Unsupported(format!("x11: window depth {depth}"))),
    };
    let bpp = setup.pixmap_formats.iter().find(|f| f.depth == depth).map(|f| f.bits_per_pixel).unwrap_or(0);
    if bpp != 32 {
        return Err(PortError::Unsupported(format!("x11: window depth {depth} with {bpp} bpp")));
    }
    let reply = conn.get_image(ImageFormat::Z_PIXMAP, window, 0, 0, width, height, !0).map_err(fail)?.reply().map_err(fail)?;
    let mut pixels = reply.data;
    if pixels.len() != usize::from(width) * usize::from(height) * 4 {
        return Err(fail(format!("unexpected image size {} for {width}x{height}", pixels.len())));
    }
    if has_alpha {
        unpremultiply(&mut pixels);
    } else {
        for px in pixels.as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
    }
    if show_cursor {
        match fetch_cursor(&conn) {
            Ok(cursor) => {
                let origin = conn.translate_coordinates(window, root, 0, 0).map_err(fail)?.reply().map_err(fail)?;
                // The hotspot is in root coordinates; the image starts at the window's own origin.
                let left = i32::from(cursor.x) - i32::from(cursor.xhot) - i32::from(origin.dst_x);
                let top = i32::from(cursor.y) - i32::from(cursor.yhot) - i32::from(origin.dst_y);
                blend_cursor(
                    &mut pixels,
                    u32::from(width),
                    u32::from(height),
                    &cursor.cursor_image,
                    u32::from(cursor.width),
                    u32::from(cursor.height),
                    left,
                    top,
                );
            }
            Err(e) => tracing::debug!("x11: cursor not drawn: {e}"),
        }
    }
    Bitmap::new(u32::from(width), u32::from(height), PixelFormat::Bgra8, Arc::from(pixels)).map_err(fail)
}

/// Turns premultiplied BGRA into straight alpha, in place. A fully transparent pixel becomes zero,
/// and a colour above its alpha (a source that was not truly premultiplied) is clamped to 255.
pub(crate) fn unpremultiply(pixels: &mut [u8]) {
    for px in pixels.as_chunks_mut::<4>().0 {
        let alpha = u32::from(px[3]);
        if alpha == 0 {
            *px = [0; 4];
            continue;
        }
        for channel in &mut px[..3] {
            *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
        }
    }
}

/// Draws the cursor onto every frame it overlaps; each frame clips it to its own bounds. A cursor
/// that cannot be read is logged and the frames go out without it.
fn draw_cursor(conn: &RustConnection, frames: &mut [(Monitor, Vec<u8>)]) {
    let cursor = match fetch_cursor(conn) {
        Ok(cursor) => cursor,
        Err(e) => {
            tracing::debug!("x11: cursor not drawn: {e}");
            return;
        }
    };
    // XFixes reports the hotspot's root position; the image's top-left corner sits `xhot, yhot` before it.
    let left = i32::from(cursor.x) - i32::from(cursor.xhot);
    let top = i32::from(cursor.y) - i32::from(cursor.yhot);
    for (monitor, pixels) in frames.iter_mut() {
        let r = monitor.rect;
        blend_cursor(
            pixels,
            r.width,
            r.height,
            &cursor.cursor_image,
            u32::from(cursor.width),
            u32::from(cursor.height),
            left - r.x,
            top - r.y,
        );
    }
}

/// XFixes refuses `GetCursorImage` until the client has negotiated a version, so the version query
/// comes first. A server without XFixes fails the query with `UnsupportedExtension`.
fn fetch_cursor(conn: &RustConnection) -> Result<GetCursorImageReply, ReplyError> {
    conn.xfixes_query_version(4, 0)?.reply()?;
    conn.xfixes_get_cursor_image()?.reply()
}

/// Alpha-blends a premultiplied ARGB cursor onto a BGRA frame. `(dx, dy)` is the cursor image's
/// top-left corner in frame pixels, and parts outside the frame are clipped. The frame's alpha is
/// left alone (captured frames are opaque).
#[allow(clippy::too_many_arguments)]
pub(crate) fn blend_cursor(
    frame_pixels_bgra: &mut [u8],
    frame_w: u32,
    frame_h: u32,
    cursor_argb: &[u32],
    cw: u32,
    ch: u32,
    dx: i32,
    dy: i32,
) {
    let (fw, fh) = (i64::from(frame_w), i64::from(frame_h));
    let (cols, rows) = (cw as usize, ch as usize);
    if cursor_argb.len() != cols * rows || frame_pixels_bgra.len() != (fw * fh * 4) as usize {
        return;
    }
    for cy in 0..rows {
        let fy = i64::from(dy) + cy as i64;
        if !(0..fh).contains(&fy) {
            continue;
        }
        for cx in 0..cols {
            let fx = i64::from(dx) + cx as i64;
            if !(0..fw).contains(&fx) {
                continue;
            }
            let src = cursor_argb[cy * cols + cx];
            let alpha = src >> 24;
            if alpha == 0 {
                continue;
            }
            let red = (src >> 16) & 0xff;
            let green = (src >> 8) & 0xff;
            let blue = src & 0xff;
            let at = (fy as usize * frame_w as usize + fx as usize) * 4;
            let px = &mut frame_pixels_bgra[at..at + 4];
            let inverse = 255 - alpha;
            px[0] = over(blue, px[0], inverse);
            px[1] = over(green, px[1], inverse);
            px[2] = over(red, px[2], inverse);
        }
    }
}

/// Premultiplied source over one opaque destination channel. The sum is clamped because a cursor
/// that is not truly premultiplied can exceed 255.
fn over(src: u32, dst: u8, inverse_alpha: u32) -> u8 {
    let scaled = (u32::from(dst) * inverse_alpha + 127) / 255;
    src.saturating_add(scaled).min(255) as u8
}

#[cfg(test)]
mod tests {
    use super::{blend_cursor, unpremultiply};

    const BLACK: [u8; 4] = [0, 0, 0, 255];
    const WHITE: [u8; 4] = [255, 255, 255, 255];

    #[test]
    fn opaque_cursor_pixel_replaces_the_frame_pixel() {
        let mut px = BLACK.repeat(4);
        blend_cursor(&mut px, 2, 2, &[0xffff_0000], 1, 1, 0, 0);
        let mut want = vec![0, 0, 255, 255];
        want.extend(BLACK.repeat(3));
        assert_eq!(px, want);
    }

    #[test]
    fn channels_are_read_by_shift_not_by_memory_layout() {
        let mut px = BLACK.to_vec();
        blend_cursor(&mut px, 1, 1, &[0xff11_2233], 1, 1, 0, 0);
        assert_eq!(px, vec![0x33, 0x22, 0x11, 0xff]);
    }

    #[test]
    fn transparent_cursor_pixel_leaves_the_frame_alone() {
        let before = [10, 20, 30, 255].repeat(4);
        let mut px = before.clone();
        blend_cursor(&mut px, 2, 2, &[0x00ff_ffff], 1, 1, 1, 1);
        assert_eq!(px, before);
    }

    #[test]
    fn half_alpha_premultiplied_gray_over_black() {
        let mut px = BLACK.to_vec();
        blend_cursor(&mut px, 1, 1, &[0x8080_8080], 1, 1, 0, 0);
        assert_eq!(px, vec![128, 128, 128, 255]);
    }

    #[test]
    fn half_alpha_black_over_white_darkens_by_half() {
        let mut px = WHITE.to_vec();
        blend_cursor(&mut px, 1, 1, &[0x8000_0000], 1, 1, 0, 0);
        assert_eq!(px, vec![127, 127, 127, 255]);
    }

    #[test]
    fn negative_offset_clips_the_left_and_top() {
        let mut px = BLACK.repeat(4);
        blend_cursor(&mut px, 2, 2, &[0xffff_ffff; 4], 2, 2, -1, -1);
        let mut want = WHITE.to_vec();
        want.extend(BLACK.repeat(3));
        assert_eq!(px, want);
    }

    #[test]
    fn overflow_past_right_and_bottom_clips_without_panic() {
        let mut px = BLACK.repeat(4);
        blend_cursor(&mut px, 2, 2, &[0xffff_ffff; 9], 3, 3, 1, 1);
        let mut want = BLACK.repeat(3);
        want.extend(WHITE);
        assert_eq!(px, want);
    }

    #[test]
    fn cursor_fully_outside_the_frame_changes_nothing() {
        let before = [1, 2, 3, 255].repeat(4);
        let mut px = before.clone();
        blend_cursor(&mut px, 2, 2, &[0xffff_ffff], 1, 1, 5, -7);
        assert_eq!(px, before);
    }

    #[test]
    fn mismatched_cursor_length_changes_nothing() {
        let before = [1, 2, 3, 255].repeat(4);
        let mut px = before.clone();
        blend_cursor(&mut px, 2, 2, &[0xffff_ffff; 3], 2, 2, 0, 0);
        assert_eq!(px, before);
    }

    #[test]
    fn opaque_pixel_is_unchanged_by_unpremultiply() {
        let mut px: Vec<u8> = vec![10, 200, 30, 255];
        unpremultiply(&mut px);
        assert_eq!(px, vec![10, 200, 30, 255]);
    }

    #[test]
    fn transparent_pixel_becomes_zero() {
        let mut px: Vec<u8> = vec![0, 0, 0, 0, 9, 8, 7, 0];
        unpremultiply(&mut px);
        assert_eq!(px, vec![0; 8]);
    }

    #[test]
    fn half_alpha_channel_is_restored() {
        // 128 straight at alpha 128 premultiplies to 64, so the round trip is exact.
        let mut px: Vec<u8> = vec![64, 64, 64, 128];
        unpremultiply(&mut px);
        assert_eq!(px, vec![128, 128, 128, 128]);
    }

    #[test]
    fn colour_above_alpha_is_clamped() {
        let mut px: Vec<u8> = vec![200, 100, 0, 100];
        unpremultiply(&mut px);
        assert_eq!(px, vec![255, 255, 0, 100]);
    }
}
