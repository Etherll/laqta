//! The mouse highlighter on X11: a click-through disc that follows the pointer.
//!
//! The window is override-redirect. It takes a 32-bit ARGB visual when the screen has one, so a
//! compositing manager can blend the translucent fill. Its input region is empty, so clicks pass
//! through, and its bounding region clips it to the disc, so no square shows without a compositor.
//! A thread polls the pointer on the root window each tick and moves the window while it is on.
//! Wayland exposes no global pointer position to clients, so the highlighter refuses to start there.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use laqta_ports::{PointerHighlighter, PortError};
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::shape::SK;
use x11rb::protocol::xfixes::ConnectionExt as _;
use x11rb::protocol::xproto::{
    Arc as XArc, Atom, AtomEnum, ChangeGCAux, ColormapAlloc, ConfigureWindowAux, ConnectionExt as _, CreateGCAux, CreateWindowAux, Depth,
    EventMask, Gcontext, PropMode, Rectangle, StackMode, VisualClass, Visualid, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{COPY_DEPTH_FROM_PARENT, COPY_FROM_PARENT, NONE};

use crate::session::Session;
use crate::x11::fail;

/// The disc's diameter in pixels. The window is a square this size.
const DIAMETER: u16 = 48;
/// The ring's width in pixels, drawn centred on the disc's edge.
const RING_WIDTH: u16 = 2;
/// The ring's centre line sits this far inside the window, so the whole stroke is visible.
const INSET: u16 = RING_WIDTH / 2;
/// The pointer is read this often while the highlighter is on.
const TICK: Duration = Duration::from_millis(16);
/// How long `set_enabled(true)` waits for the window thread to report back.
const START_TIMEOUT: Duration = Duration::from_secs(5);
/// The fill's alpha when a compositor blends it: 0.35 of 255, rounded.
const FILL_ALPHA: u8 = 89;
const FILL_RGB: (u8, u8, u8) = (255, 255, 0);
const RING_RGB: (u8, u8, u8) = (255, 165, 0);
/// The channel masks of a 32-bit ARGB visual.
const ARGB_MASKS: (u32, u32, u32) = (0xff_0000, 0xff_00, 0xff);
/// The XFixes version to negotiate. `capture.rs` asks for the same.
const XFIXES_VERSION: (u32, u32) = (4, 0);
/// A full turn in X11's 1/64 degree units.
const FULL_TURN: i16 = 360 * 64;

/// The mouse highlighter for the session `main` detected.
#[derive(Debug)]
pub struct LinuxHighlighter {
    session: Session,
    running: Mutex<Option<Running>>,
}

/// The thread that owns the overlay while the highlighter is on.
#[derive(Debug)]
struct Running {
    flag: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

impl LinuxHighlighter {
    pub fn new(session: Session) -> Self {
        Self { session, running: Mutex::new(None) }
    }

    fn lock(&self) -> MutexGuard<'_, Option<Running>> {
        self.running.lock().unwrap_or_else(|p| p.into_inner())
    }
}

impl Drop for LinuxHighlighter {
    fn drop(&mut self) {
        let _ = self.set_enabled(false);
    }
}

impl PointerHighlighter for LinuxHighlighter {
    fn set_enabled(&self, on: bool) -> Result<(), PortError> {
        let mut running = self.lock();
        if let Some(current) = running.take() {
            // A thread that has exited (say, the X connection dropped) counts as off, so turning on
            // restarts it.
            if on && !current.thread.is_finished() {
                *running = Some(current);
                return Ok(());
            }
            current.stop();
        }
        if !on {
            return Ok(());
        }
        if self.session != Session::X11 {
            return Err(PortError::Unsupported("the mouse highlighter needs X11 or Windows".into()));
        }
        *running = Some(Running::start()?);
        Ok(())
    }

    fn is_enabled(&self) -> bool {
        self.lock().as_ref().is_some_and(|running| !running.thread.is_finished())
    }
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

/// The thread body: opens the overlay, reports the result, then follows the pointer until the flag
/// is set. The overlay is destroyed when this function returns.
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

/// The overlay window with its own connection, graphics context and pixel values.
struct Overlay {
    conn: RustConnection,
    root: Window,
    window: Window,
    gc: Gcontext,
    fill: u32,
    ring: u32,
    /// Where the window was last moved, so an idle pointer costs no requests.
    at: Option<(i32, i32)>,
}

impl Overlay {
    fn open() -> Result<Self, PortError> {
        let (conn, screen_num) = x11rb::connect(None).map_err(fail)?;
        let screen = &conn.setup().roots[screen_num];
        let root = screen.root;
        let argb = argb_visual(&screen.allowed_depths);
        let composited = argb.is_some() && compositor_running(&conn)?;
        let (depth, visual, masks) = match argb {
            Some(visual) => (32, visual, ARGB_MASKS),
            None => {
                let masks = visual_masks(&screen.allowed_depths, screen.root_visual)
                    .ok_or_else(|| PortError::Failed("x11: the root visual is missing from the depths".into()))?;
                (COPY_DEPTH_FROM_PARENT, COPY_FROM_PARENT, masks)
            }
        };
        let alpha = if composited { FILL_ALPHA } else { u8::MAX };
        let fill = pixel(argb.is_some(), masks, alpha, FILL_RGB);
        let ring = pixel(argb.is_some(), masks, u8::MAX, RING_RGB);

        // A 32-bit visual is not the parent's, so the window needs its own colormap and border pixel.
        let colormap = match argb {
            Some(visual) => {
                let colormap = conn.generate_id().map_err(fail)?;
                conn.create_colormap(ColormapAlloc::NONE, colormap, root, visual).map_err(fail)?.check().map_err(fail)?;
                colormap
            }
            None => COPY_FROM_PARENT,
        };
        let window = conn.generate_id().map_err(fail)?;
        let aux = CreateWindowAux::new()
            .background_pixel(0)
            .border_pixel(0)
            .colormap(colormap)
            .override_redirect(1)
            .event_mask(EventMask::EXPOSURE);
        conn.create_window(depth, window, root, 0, 0, DIAMETER, DIAMETER, 0, WindowClass::INPUT_OUTPUT, visual, &aux)
            .map_err(fail)?
            .check()
            .map_err(fail)?;

        // A window manager ignores this on an override-redirect window. It is set for the spec, and the
        // stacking comes from the restack in `step`.
        let net_wm_state = intern(&conn, b"_NET_WM_STATE")?;
        let above = intern(&conn, b"_NET_WM_STATE_ABOVE")?;
        conn.change_property32(PropMode::REPLACE, window, net_wm_state, AtomEnum::ATOM, &[above]).map_err(fail)?.check().map_err(fail)?;

        shape(&conn, window)?;

        let gc = conn.generate_id().map_err(fail)?;
        conn.create_gc(gc, window, &CreateGCAux::new().foreground(fill).line_width(u32::from(RING_WIDTH)))
            .map_err(fail)?
            .check()
            .map_err(fail)?;
        conn.map_window(window).map_err(fail)?.check().map_err(fail)?;
        conn.flush().map_err(fail)?;
        Ok(Self { conn, root, window, gc, fill, ring, at: None })
    }

    /// Redraws on expose, and moves the window to the pointer when the pointer has moved.
    fn step(&mut self) -> Result<(), PortError> {
        while let Some(event) = self.conn.poll_for_event().map_err(fail)? {
            match event {
                Event::Expose(expose) if expose.count == 0 => self.draw()?,
                // A failed request means the window can no longer be trusted: stop, so `is_enabled` reports off.
                Event::Error(error) => return Err(fail(format!("{error:?}"))),
                _ => {}
            }
        }
        let pointer = self.conn.query_pointer(self.root).map_err(fail)?.reply().map_err(fail)?;
        let origin = window_origin((i32::from(pointer.root_x), i32::from(pointer.root_y)), i32::from(DIAMETER));
        if self.at != Some(origin) {
            let (x, y) = origin;
            self.conn.configure_window(self.window, &ConfigureWindowAux::new().x(x).y(y).stack_mode(StackMode::ABOVE)).map_err(fail)?;
            self.at = Some(origin);
            self.conn.flush().map_err(fail)?;
        }
        Ok(())
    }

    fn draw(&self) -> Result<(), PortError> {
        self.conn.change_gc(self.gc, &ChangeGCAux::new().foreground(self.fill)).map_err(fail)?;
        self.conn.poly_fill_arc(self.window, self.gc, &[disc_arc()]).map_err(fail)?;
        self.conn.change_gc(self.gc, &ChangeGCAux::new().foreground(self.ring)).map_err(fail)?;
        self.conn.poly_arc(self.window, self.gc, &[ring_arc()]).map_err(fail)?;
        self.conn.flush().map_err(fail)
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        let _ = self.conn.destroy_window(self.window);
        let _ = self.conn.free_gc(self.gc);
        let _ = self.conn.flush();
    }
}

/// Clips the window to the disc, and gives it an empty input region so the pointer passes through.
fn shape(conn: &RustConnection, window: Window) -> Result<(), PortError> {
    conn.xfixes_query_version(XFIXES_VERSION.0, XFIXES_VERSION.1).map_err(fail)?.reply().map_err(fail)?;
    let input = conn.generate_id().map_err(fail)?;
    conn.xfixes_create_region(input, &[]).map_err(fail)?.check().map_err(fail)?;
    conn.xfixes_set_window_shape_region(window, SK::INPUT, 0, 0, input).map_err(fail)?.check().map_err(fail)?;
    conn.xfixes_destroy_region(input).map_err(fail)?.check().map_err(fail)?;

    let bounding = conn.generate_id().map_err(fail)?;
    conn.xfixes_create_region(bounding, &disc_rects(DIAMETER)).map_err(fail)?.check().map_err(fail)?;
    conn.xfixes_set_window_shape_region(window, SK::BOUNDING, 0, 0, bounding).map_err(fail)?.check().map_err(fail)?;
    conn.xfixes_destroy_region(bounding).map_err(fail)?.check().map_err(fail)?;
    Ok(())
}

/// Whether a compositing manager owns the screen's `_NET_WM_CM_S0` selection. Only then is the
/// fill's alpha blended; without one the translucent fill would show as a darkened colour.
fn compositor_running(conn: &RustConnection) -> Result<bool, PortError> {
    let selection = intern(conn, b"_NET_WM_CM_S0")?;
    let owner = conn.get_selection_owner(selection).map_err(fail)?.reply().map_err(fail)?.owner;
    Ok(owner != NONE)
}

/// The pixel for a colour: premultiplied when the window has alpha, else scaled into the masks.
fn pixel(argb: bool, masks: (u32, u32, u32), alpha: u8, rgb: (u8, u8, u8)) -> u32 {
    if argb { premultiplied(alpha, rgb) } else { opaque_pixel(rgb, masks) }
}

/// A premultiplied ARGB32 pixel: each colour channel is scaled by the alpha.
fn premultiplied(alpha: u8, (red, green, blue): (u8, u8, u8)) -> u32 {
    let scale = |channel: u8| (u32::from(channel) * u32::from(alpha) + 127) / 255;
    (u32::from(alpha) << 24) | (scale(red) << 16) | (scale(green) << 8) | scale(blue)
}

/// The pixel for an opaque colour on a visual with these channel masks (red, green, blue).
fn opaque_pixel((red, green, blue): (u8, u8, u8), (red_mask, green_mask, blue_mask): (u32, u32, u32)) -> u32 {
    place(red, red_mask) | place(green, green_mask) | place(blue, blue_mask)
}

/// A channel value spread over the bits of `mask`.
fn place(channel: u8, mask: u32) -> u32 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let levels = mask >> shift;
    let scaled = (u64::from(channel) * u64::from(levels) + 127) / 255;
    (scaled as u32) << shift
}

/// The first 32-bit TrueColor visual with 8-bit RGB channels, which can hold alpha.
fn argb_visual(depths: &[Depth]) -> Option<Visualid> {
    depths
        .iter()
        .filter(|depth| depth.depth == 32)
        .flat_map(|depth| &depth.visuals)
        .find(|visual| visual.class == VisualClass::TRUE_COLOR && (visual.red_mask, visual.green_mask, visual.blue_mask) == ARGB_MASKS)
        .map(|visual| visual.visual_id)
}

/// The channel masks of `visual`, when the screen lists it.
fn visual_masks(depths: &[Depth], visual: Visualid) -> Option<(u32, u32, u32)> {
    depths
        .iter()
        .flat_map(|depth| &depth.visuals)
        .find(|candidate| candidate.visual_id == visual)
        .map(|found| (found.red_mask, found.green_mask, found.blue_mask))
}

/// The top-left corner that centres a square of `size` on the pointer.
fn window_origin(pointer: (i32, i32), size: i32) -> (i32, i32) {
    (pointer.0 - size / 2, pointer.1 - size / 2)
}

/// The disc, filling the window.
fn disc_arc() -> XArc {
    XArc { x: 0, y: 0, width: DIAMETER, height: DIAMETER, angle1: 0, angle2: FULL_TURN }
}

/// The ring's centre line. With the line `RING_WIDTH` wide, the stroke covers the window's edge.
fn ring_arc() -> XArc {
    XArc { x: INSET as i16, y: INSET as i16, width: DIAMETER - 2 * INSET, height: DIAMETER - 2 * INSET, angle1: 0, angle2: FULL_TURN }
}

/// The disc as one-pixel-high rectangles: the row-by-row run of pixels whose centres lie inside the
/// circle. A convex disc has one run per row.
fn disc_rects(diameter: u16) -> Vec<Rectangle> {
    let radius = f64::from(diameter) / 2.0;
    (0..diameter)
        .filter_map(|y| {
            let dy = f64::from(y) + 0.5 - radius;
            let inside: Vec<u16> = (0..diameter)
                .filter(|&x| {
                    let dx = f64::from(x) + 0.5 - radius;
                    dx * dx + dy * dy <= radius * radius
                })
                .collect();
            let (first, last) = (*inside.first()?, *inside.last()?);
            Some(Rectangle { x: first as i16, y: y as i16, width: last - first + 1, height: 1 })
        })
        .collect()
}

fn intern(conn: &RustConnection, name: &[u8]) -> Result<Atom, PortError> {
    Ok(conn.intern_atom(false, name).map_err(fail)?.reply().map_err(fail)?.atom)
}

#[cfg(test)]
mod tests {
    use laqta_ports::{PointerHighlighter, PortError};
    use x11rb::protocol::xproto::{Depth, VisualClass, Visualtype};

    use super::{
        ARGB_MASKS, FILL_RGB, LinuxHighlighter, RING_RGB, Session, argb_visual, disc_rects, opaque_pixel, pixel, premultiplied,
        visual_masks, window_origin,
    };

    fn visual(id: u32, class: VisualClass, masks: (u32, u32, u32)) -> Visualtype {
        Visualtype {
            visual_id: id,
            class,
            bits_per_rgb_value: 8,
            colormap_entries: 256,
            red_mask: masks.0,
            green_mask: masks.1,
            blue_mask: masks.2,
        }
    }

    fn depth(depth: u8, visuals: Vec<Visualtype>) -> Depth {
        Depth { depth, visuals }
    }

    #[test]
    fn premultiplied_scales_the_colour_by_alpha() {
        assert_eq!(premultiplied(255, (255, 255, 0)), 0xffff_ff00);
        assert_eq!(premultiplied(89, (255, 255, 0)), 0x5959_5900);
        assert_eq!(premultiplied(0, (255, 165, 0)), 0);
    }

    #[test]
    fn argb_windows_use_premultiplied_pixels_and_others_use_the_masks() {
        assert_eq!(pixel(true, ARGB_MASKS, 89, FILL_RGB), 0x5959_5900);
        assert_eq!(pixel(true, ARGB_MASKS, 255, RING_RGB), 0xffff_a500);
        assert_eq!(pixel(false, (0xff_0000, 0xff_00, 0xff), 89, FILL_RGB), 0x00ff_ff00);
    }

    #[test]
    fn opaque_pixels_follow_the_channel_masks() {
        assert_eq!(opaque_pixel((255, 255, 255), (0xf800, 0x07e0, 0x001f)), 0xffff);
        assert_eq!(opaque_pixel((0, 0, 0), (0xf800, 0x07e0, 0x001f)), 0);
    }

    #[test]
    fn the_argb_visual_is_a_32_bit_truecolor_with_8_bit_channels() {
        let depths = [
            depth(24, vec![visual(0x21, VisualClass::TRUE_COLOR, ARGB_MASKS)]),
            depth(32, vec![visual(0x30, VisualClass::TRUE_COLOR, ARGB_MASKS)]),
        ];
        assert_eq!(argb_visual(&depths), Some(0x30));
        assert_eq!(argb_visual(&depths[..1]), None);
        let direct = [depth(32, vec![visual(0x31, VisualClass::DIRECT_COLOR, ARGB_MASKS)])];
        assert_eq!(argb_visual(&direct), None);
        let bgr = [depth(32, vec![visual(0x32, VisualClass::TRUE_COLOR, (0xff, 0xff_00, 0xff_0000))])];
        assert_eq!(argb_visual(&bgr), None);
    }

    #[test]
    fn a_visual_is_found_by_id_across_depths() {
        let depths = [depth(24, vec![visual(0x21, VisualClass::TRUE_COLOR, (0xff_0000, 0xff_00, 0xff))])];
        assert_eq!(visual_masks(&depths, 0x21), Some((0xff_0000, 0xff_00, 0xff)));
        assert_eq!(visual_masks(&depths, 0x99), None);
    }

    #[test]
    fn the_window_is_centred_on_the_pointer() {
        assert_eq!(window_origin((100, 50), 48), (76, 26));
        assert_eq!(window_origin((0, 0), 48), (-24, -24));
    }

    #[test]
    fn the_disc_is_one_run_per_row_and_symmetric() {
        let rects = disc_rects(48);
        assert_eq!(rects.len(), 48);
        let middle = &rects[24];
        assert_eq!((middle.x, middle.y, middle.width, middle.height), (0, 24, 48, 1));
        for (top, bottom) in rects.iter().zip(rects.iter().rev()) {
            assert_eq!(top.width, bottom.width);
            assert_eq!(top.x, bottom.x);
        }
        let covered: u32 = rects.iter().map(|rect| u32::from(rect.width)).sum();
        assert!((1780..=1830).contains(&covered), "{covered}");
    }

    #[test]
    fn wayland_refuses_to_start_and_turning_off_is_a_no_op() {
        let highlighter = LinuxHighlighter::new(Session::Wayland);
        assert!(
            matches!(highlighter.set_enabled(true), Err(PortError::Unsupported(msg)) if msg == "the mouse highlighter needs X11 or Windows")
        );
        assert!(!highlighter.is_enabled());
        assert!(highlighter.set_enabled(false).is_ok());
    }
}
