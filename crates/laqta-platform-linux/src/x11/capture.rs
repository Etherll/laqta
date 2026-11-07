use std::sync::Arc;

use laqta_domain::{Bitmap, Frame, Monitor, MonitorId, PhysRect, PixelFormat};
use laqta_ports::{BoxFuture, PortError, ScreenSource};
use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat};

/// Captures each RandR monitor from the root window with `GetImage`.
///
/// Decision (M0): plain `GetImage` only; the XShm path is deferred to M1, where the overlay latency
/// budget measures whether it is needed.
#[derive(Debug)]
pub struct X11Screen;

impl ScreenSource for X11Screen {
    fn capture_all(&self) -> BoxFuture<'_, Result<Vec<Frame>, PortError>> {
        Box::pin(blocking::unblock(capture_all))
    }
}

fn fail(e: impl std::fmt::Display) -> PortError {
    PortError::Failed(format!("x11: {e}"))
}

pub(crate) fn capture_all() -> Result<Vec<Frame>, PortError> {
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
    let mut frames = Vec::with_capacity(monitors.len());
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
        let image = Bitmap::new(r.width, r.height, PixelFormat::Bgra8, Arc::from(data)).map_err(fail)?;
        frames.push(Frame { monitor, image });
    }
    Ok(frames)
}
