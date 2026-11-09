//! Synthetic input for scrolling capture on X11: the pointer moves to the region's point,
//! then wheel notches or arrow and page keys go through XTEST `fake_input`. Wayland has no client
//! input injection (it needs the RemoteDesktop portal), so every call there is `Unsupported`.

use laqta_domain::capture::ScrollMethod;
use laqta_ports::{InputSynth, PortError};
use x11rb::NONE;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, ConnectionExt as _, KEY_PRESS_EVENT, KEY_RELEASE_EVENT, Keycode, Keysym, MOTION_NOTIFY_EVENT,
    Window,
};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

use crate::session::Session;
use crate::x11::fail;

/// Button 5 is the wheel down; button 4 is up.
const WHEEL_DOWN: u8 = 5;
/// Keysyms of the keys the key scroll methods press (`keysymdef.h`).
const XK_DOWN: Keysym = 0xff54;
const XK_NEXT: Keysym = 0xff56;
/// XTEST `detail` of an absolute `MotionNotify`.
const ABSOLUTE: u8 = 0;
/// XTEST `time` for "now".
const CURRENT_TIME: u32 = 0;

/// Synthetic input for the session `main` detected.
#[derive(Debug)]
pub struct LinuxInput {
    session: Session,
}

impl LinuxInput {
    pub fn new(session: Session) -> Self {
        Self { session }
    }
}

impl InputSynth for LinuxInput {
    fn scroll(&self, x: i32, y: i32, method: ScrollMethod, amount: u32) -> Result<(), PortError> {
        if self.session != Session::X11 {
            return Err(PortError::Unsupported("synthetic input needs X11 or Windows".into()));
        }
        let key = scroll_keysym(method)?;
        let (root_x, root_y) = (root_coord(x)?, root_coord(y)?);
        let (conn, screen) = x11rb::connect(None).map_err(fail)?;
        let root = conn.setup().roots[screen].root;
        // Looked up before the pointer moves, so a keyboard without the key sends nothing.
        let keycode = key.map(|sym| keycode_of(&conn, sym)).transpose()?;

        fake_event(&conn, MOTION_NOTIFY_EVENT, ABSOLUTE, root, root_x, root_y)?;
        for _ in 0..amount {
            match keycode {
                None => {
                    fake_event(&conn, BUTTON_PRESS_EVENT, WHEEL_DOWN, NONE, 0, 0)?;
                    fake_event(&conn, BUTTON_RELEASE_EVENT, WHEEL_DOWN, NONE, 0, 0)?;
                }
                Some(code) => {
                    fake_event(&conn, KEY_PRESS_EVENT, code, NONE, 0, 0)?;
                    fake_event(&conn, KEY_RELEASE_EVENT, code, NONE, 0, 0)?;
                }
            }
        }
        Ok(())
    }
}

/// The keysym a key scroll method presses per step, or `None` for the wheel.
fn scroll_keysym(method: ScrollMethod) -> Result<Option<Keysym>, PortError> {
    match method {
        ScrollMethod::MouseWheel => Ok(None),
        ScrollMethod::DownArrow => Ok(Some(XK_DOWN)),
        ScrollMethod::PageDown => Ok(Some(XK_NEXT)),
        ScrollMethod::ScrollMessage => Err(PortError::Unsupported("scroll messages are Windows-only".into())),
    }
}

/// A root-window coordinate. The protocol carries 16-bit values, so a larger desktop cannot be reached.
fn root_coord(value: i32) -> Result<i16, PortError> {
    i16::try_from(value).map_err(|_| PortError::Failed(format!("x11: coordinate {value} is outside the 16-bit root space")))
}

/// The keycode that produces `keysym` on the current keyboard map.
fn keycode_of(conn: &RustConnection, keysym: Keysym) -> Result<Keycode, PortError> {
    let setup = conn.setup();
    let (min, max) = (setup.min_keycode, setup.max_keycode);
    let mapping = conn.get_keyboard_mapping(min, max - min + 1).map_err(fail)?.reply().map_err(fail)?;
    keycode_for(&mapping.keysyms, mapping.keysyms_per_keycode, min, keysym)
        .ok_or_else(|| PortError::Failed(format!("x11: no keycode for keysym {keysym:#x}")))
}

/// The keycode whose row of `per` keysyms holds `keysym`. `keysyms` is the flat list
/// `GetKeyboardMapping` returns, starting at keycode `min`.
fn keycode_for(keysyms: &[Keysym], per: u8, min: Keycode, keysym: Keysym) -> Option<Keycode> {
    let row = keysyms.chunks(usize::from(per.max(1))).position(|row| row.contains(&keysym))?;
    min.checked_add(u8::try_from(row).ok()?)
}

/// Sends one XTEST event and waits for the server to take it. `check` also flushes the connection.
fn fake_event(conn: &RustConnection, kind: u8, detail: u8, root: Window, x: i16, y: i16) -> Result<(), PortError> {
    conn.xtest_fake_input(kind, detail, CURRENT_TIME, root, x, y, 0).map_err(fail)?.check().map_err(fail)
}

#[cfg(test)]
mod tests {
    use laqta_domain::capture::ScrollMethod;
    use laqta_ports::{InputSynth, PortError};

    use super::{LinuxInput, Session, keycode_for, root_coord, scroll_keysym};

    /// Keycodes 8, 9 and 10, two keysyms each: 'b' is on 9, 'c' on 10.
    const MAP: [u32; 6] = [0x61, 0x41, 0x62, 0x42, 0x63, 0x43];

    #[test]
    fn key_methods_press_down_and_next_and_the_wheel_presses_no_key() {
        assert_eq!(scroll_keysym(ScrollMethod::MouseWheel).ok(), Some(None));
        assert_eq!(scroll_keysym(ScrollMethod::DownArrow).ok(), Some(Some(0xff54)));
        assert_eq!(scroll_keysym(ScrollMethod::PageDown).ok(), Some(Some(0xff56)));
    }

    #[test]
    fn scroll_messages_are_refused_on_x11() {
        assert!(matches!(scroll_keysym(ScrollMethod::ScrollMessage), Err(PortError::Unsupported(msg)) if msg.contains("Windows-only")));
        let refused = LinuxInput::new(Session::X11).scroll(0, 0, ScrollMethod::ScrollMessage, 1);
        assert!(matches!(refused, Err(PortError::Unsupported(msg)) if msg.contains("Windows-only")));
    }

    #[test]
    fn wayland_refuses_every_method_before_touching_x11() {
        for method in [ScrollMethod::MouseWheel, ScrollMethod::DownArrow, ScrollMethod::PageDown, ScrollMethod::ScrollMessage] {
            let refused = LinuxInput::new(Session::Wayland).scroll(10, 10, method, 2);
            assert!(matches!(refused, Err(PortError::Unsupported(msg)) if msg == "synthetic input needs X11 or Windows"));
        }
    }

    #[test]
    fn keycode_is_found_by_row_and_offset_from_the_first_keycode() {
        assert_eq!(keycode_for(&MAP, 2, 8, 0x42), Some(9));
        assert_eq!(keycode_for(&MAP, 2, 8, 0x63), Some(10));
        assert_eq!(keycode_for(&MAP, 2, 100, 0x63), Some(102));
    }

    #[test]
    fn unmapped_keysym_has_no_keycode() {
        assert_eq!(keycode_for(&MAP, 2, 8, 0xff54), None);
    }

    #[test]
    fn zero_keysyms_per_keycode_reads_as_one() {
        assert_eq!(keycode_for(&[1, 2, 3], 0, 8, 2), Some(9));
    }

    #[test]
    fn keycode_past_the_last_byte_is_refused() {
        assert_eq!(keycode_for(&[1, 2], 1, 255, 2), None);
    }

    #[test]
    fn root_coordinates_must_fit_sixteen_bits() {
        assert_eq!(root_coord(-32768).ok(), Some(i16::MIN));
        assert_eq!(root_coord(32767).ok(), Some(i16::MAX));
        assert!(root_coord(32768).is_err());
        assert!(root_coord(-32769).is_err());
    }
}
