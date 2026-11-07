//! Window tools on X11 (M5 D7): borderless through `_MOTIF_WM_HINTS`, top-most through a
//! `_NET_WM_STATE` client message to the root window, and the inspector's properties. Wayland has
//! no client-side window control, so every call there is `Unsupported`.

use laqta_domain::region::HoverTarget;
use laqta_ports::{PortError, WindowOps};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{Atom, AtomEnum, ClientMessageEvent, ConnectionExt as _, EventMask, PropMode, Window};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use crate::session::Session;
use crate::x11::fail;
use crate::x11::windows::parse_wm_class;

/// `_MOTIF_WM_HINTS` flags: only the `decorations` field is set.
const MWM_HINTS_DECORATIONS: u32 = 1 << 1;
/// `_NET_WM_STATE` actions.
const STATE_REMOVE: u32 = 0;
const STATE_ADD: u32 = 1;
/// `_NET_WM_STATE` source indication: a normal application.
const SOURCE_APPLICATION: u32 = 1;
/// Property reads ask for up to this many 32-bit units. The server returns the real length.
const LONG_LEN: u32 = 1 << 20;

/// Window tools for the session `main` detected.
#[derive(Debug)]
pub struct LinuxWindowOps {
    session: Session,
}

impl LinuxWindowOps {
    pub fn new(session: Session) -> Self {
        Self { session }
    }
}

impl WindowOps for LinuxWindowOps {
    fn set_borderless(&self, window: &HoverTarget, borderless: bool) -> Result<(), PortError> {
        let win = x11_window(self.session, window)?;
        let link = Link::open()?;
        let decorations = if borderless { 0 } else { 1 };
        let request =
            link.conn.change_property32(PropMode::REPLACE, win, link.motif, link.motif, &motif_hints(decorations)).map_err(fail)?;
        request.check().map_err(fail)
    }

    fn set_topmost(&self, window: &HoverTarget, topmost: bool) -> Result<(), PortError> {
        let win = x11_window(self.session, window)?;
        let link = Link::open()?;
        let event = ClientMessageEvent::new(32, win, link.net_wm_state, state_message(topmost, link.above));
        let mask = EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY;
        let request = link.conn.send_event(false, link.root, mask, event).map_err(fail)?;
        request.check().map_err(fail)
    }

    fn inspect(&self, window: &HoverTarget) -> Result<Vec<(String, String)>, PortError> {
        let win = x11_window(self.session, window)?;
        let link = Link::open()?;
        let (instance, class) = parse_wm_class(&link.bytes(win, AtomEnum::WM_CLASS, AtomEnum::ANY)?.unwrap_or_default());
        let title = link.bytes(win, link.net_wm_name, link.utf8)?.unwrap_or_default();
        let pid = link.u32s(win, link.net_wm_pid)?.and_then(|pids| pids.first().copied());
        let states = link.u32s(win, link.net_wm_state)?.unwrap_or_default();
        let state_names = states.iter().map(|atom| link.atom_name(*atom)).collect::<Result<Vec<_>, _>>()?;
        let attrs = link.conn.get_window_attributes(win).map_err(fail)?.reply().map_err(fail)?;
        let geometry = link.conn.get_geometry(win).map_err(fail)?.reply().map_err(fail)?;
        let origin = link.conn.translate_coordinates(win, link.root, 0, 0).map_err(fail)?.reply().map_err(fail)?;
        Ok(vec![
            ("id".to_owned(), format!("{win:#x}")),
            ("WM_CLASS".to_owned(), format!("{instance}, {class}")),
            ("_NET_WM_NAME".to_owned(), String::from_utf8_lossy(&title).into_owned()),
            ("_NET_WM_PID".to_owned(), pid.map_or_else(|| "none".to_owned(), |pid| pid.to_string())),
            ("geometry".to_owned(), format!("{}, {}, {}x{}", origin.dst_x, origin.dst_y, geometry.width, geometry.height)),
            ("_NET_WM_STATE".to_owned(), if state_names.is_empty() { "none".to_owned() } else { state_names.join(", ") }),
            ("override-redirect".to_owned(), attrs.override_redirect.to_string()),
        ])
    }
}

/// The X11 window a hover target names. Wayland targets are sway con ids, so they are refused even
/// under XWayland. A zero or out-of-range handle names no window.
fn x11_window(session: Session, target: &HoverTarget) -> Result<Window, PortError> {
    if session != Session::X11 {
        return Err(PortError::Unsupported("window tools need an X11 session".into()));
    }
    u32::try_from(target.handle)
        .ok()
        .filter(|id| *id != 0)
        .ok_or_else(|| PortError::Failed(format!("not an X11 window id: {}", target.handle)))
}

/// The `_MOTIF_WM_HINTS` value: `flags, functions, decorations, input_mode, status`.
fn motif_hints(decorations: u32) -> [u32; 5] {
    [MWM_HINTS_DECORATIONS, 0, decorations, 0, 0]
}

/// The data of a `_NET_WM_STATE` client message that adds or removes `above`.
fn state_message(add: bool, above: Atom) -> [u32; 5] {
    [if add { STATE_ADD } else { STATE_REMOVE }, above, 0, SOURCE_APPLICATION, 0]
}

/// One connection, the root window and the atoms these tools use. Opened per call, as the hover
/// lookups do.
struct Link {
    conn: RustConnection,
    root: Window,
    motif: Atom,
    net_wm_state: Atom,
    above: Atom,
    net_wm_name: Atom,
    utf8: Atom,
    net_wm_pid: Atom,
}

impl Link {
    fn open() -> Result<Self, PortError> {
        let (conn, screen) = x11rb::connect(None).map_err(fail)?;
        let root = conn.setup().roots[screen].root;
        Ok(Self {
            motif: intern(&conn, b"_MOTIF_WM_HINTS")?,
            net_wm_state: intern(&conn, b"_NET_WM_STATE")?,
            above: intern(&conn, b"_NET_WM_STATE_ABOVE")?,
            net_wm_name: intern(&conn, b"_NET_WM_NAME")?,
            utf8: intern(&conn, b"UTF8_STRING")?,
            net_wm_pid: intern(&conn, b"_NET_WM_PID")?,
            root,
            conn,
        })
    }

    /// A property's 32-bit values, or None when it is missing or has another format.
    fn u32s(&self, win: Window, prop: impl Into<Atom>) -> Result<Option<Vec<u32>>, PortError> {
        let reply = self.conn.get_property(false, win, prop, AtomEnum::ANY, 0, LONG_LEN).map_err(fail)?.reply().map_err(fail)?;
        Ok(reply.value32().map(|values| values.collect()))
    }

    /// A property's bytes when it has 8-bit format, or None otherwise.
    fn bytes(&self, win: Window, prop: impl Into<Atom>, ty: impl Into<Atom>) -> Result<Option<Vec<u8>>, PortError> {
        let reply = self.conn.get_property(false, win, prop, ty, 0, LONG_LEN).map_err(fail)?.reply().map_err(fail)?;
        Ok((reply.format == 8).then_some(reply.value))
    }

    fn atom_name(&self, atom: Atom) -> Result<String, PortError> {
        let reply = self.conn.get_atom_name(atom).map_err(fail)?.reply().map_err(fail)?;
        Ok(String::from_utf8_lossy(&reply.name).into_owned())
    }
}

fn intern(conn: &RustConnection, name: &[u8]) -> Result<Atom, PortError> {
    Ok(conn.intern_atom(false, name).map_err(fail)?.reply().map_err(fail)?.atom)
}

#[cfg(test)]
mod tests {
    use laqta_domain::PhysRect;
    use laqta_domain::region::{HoverTarget, TargetKind};
    use laqta_ports::PortError;

    use super::{Session, motif_hints, state_message, x11_window};

    fn target(handle: u64) -> HoverTarget {
        HoverTarget {
            rect: PhysRect { x: 0, y: 0, width: 10, height: 10 },
            handle,
            title: String::new(),
            process: String::new(),
            kind: TargetKind::Window,
        }
    }

    #[test]
    fn motif_hints_set_only_the_decorations_flag() {
        assert_eq!(motif_hints(0), [2, 0, 0, 0, 0]);
        assert_eq!(motif_hints(1), [2, 0, 1, 0, 0]);
    }

    #[test]
    fn state_message_adds_or_removes_the_above_state() {
        assert_eq!(state_message(true, 77), [1, 77, 0, 1, 0]);
        assert_eq!(state_message(false, 77), [0, 77, 0, 1, 0]);
    }

    #[test]
    fn wayland_refuses_window_tools_even_with_a_window_id() {
        assert!(matches!(x11_window(Session::Wayland, &target(0x42)), Err(PortError::Unsupported(_))));
    }

    #[test]
    fn x11_handle_must_be_a_nonzero_window_id() {
        assert!(matches!(x11_window(Session::X11, &target(0)), Err(PortError::Failed(_))));
        assert!(matches!(x11_window(Session::X11, &target(u64::from(u32::MAX) + 1)), Err(PortError::Failed(_))));
        assert_eq!(x11_window(Session::X11, &target(0x42)).ok(), Some(0x42));
    }
}
