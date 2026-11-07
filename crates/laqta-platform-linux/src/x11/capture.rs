use std::sync::Arc;

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
/// Decision (M0): plain `GetImage` only; the XShm path is deferred to M1, where the overlay latency
/// budget measures whether it is needed.
#[derive(Debug)]
pub struct X11Screen;

impl ScreenSource for X11Screen {
    fn capture_all(&self) -> BoxFuture<'_, Result<Vec<Frame>, PortError>> {
        Box::pin(blocking::unblock(|| capture_frames(false)))
    }

    fn capture(&self, request: CaptureRequest) -> BoxFuture<'_, Result<Vec<Frame>, PortError>> {
        Box::pin(blocking::unblock(move || capture_frames(request.show_cursor)))
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
                let name = conn.get_atom_name(m.name).ok().and_then(|c| c.reply().ok()).map(|r| String::from_utf8_lossy(&r.name).into_owned()).unwrap_or_default();
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
pub(crate) fn blend_cursor(frame_pixels_bgra: &mut [u8], frame_w: u32, frame_h: u32, cursor_argb: &[u32], cw: u32, ch: u32, dx: i32, dy: i32) {
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
    use super::blend_cursor;

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
}
