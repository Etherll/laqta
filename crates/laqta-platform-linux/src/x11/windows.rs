//! EWMH window lookups for X11 hover targets and the active window. The byte and rect helpers are
//! pure, so their tests need no display.

use std::fs;

use laqta_domain::PhysRect;
use laqta_domain::region::{HoverTarget, TargetKind};
use laqta_ports::PortError;
use x11rb::connection::Connection;
use x11rb::errors::ReplyError;
use x11rb::protocol::xproto::{Atom, AtomEnum, ConnectionExt as _, MapState, Window};
use x11rb::rust_connection::RustConnection;

use super::fail;

/// Property reads ask for up to this many 32-bit units. The server returns the property's real
/// length, so this only bounds a malformed property.
const LONG_LEN: u32 = 1 << 20;

/// Every window the window manager lists, front to back, with its usable rect and labels. The
/// stacking list is the first choice, with `_NET_CLIENT_LIST` as the fallback.
pub(crate) fn targets() -> Result<Vec<HoverTarget>, PortError> {
    let scan = Scan::open()?;
    let listed = scan.root_windows()?.ok_or_else(|| PortError::Unsupported("x11: no EWMH client list".into()))?;
    let mut out = Vec::new();
    for win in front_to_back(listed) {
        match scan.target(win) {
            Ok(Some(target)) => out.push(target),
            Ok(None) => {}
            // The window closed after the list was read, so it is no longer a target.
            Err(ReplyError::X11Error(_)) => {}
            Err(e) => return Err(fail(e)),
        }
    }
    Ok(out)
}

/// The focused top-level window, from `_NET_ACTIVE_WINDOW`. Missing or zero means none.
pub(crate) fn active_window() -> Result<Option<HoverTarget>, PortError> {
    let scan = Scan::open()?;
    let active = scan.u32s(scan.root, scan.atoms.active_window).map_err(fail)?;
    let Some(win) = active.and_then(|ids| ids.first().copied()).filter(|w| *w != 0) else {
        return Ok(None);
    };
    match scan.target(win) {
        Ok(target) => Ok(target),
        Err(ReplyError::X11Error(_)) => Ok(None),
        Err(e) => Err(fail(e)),
    }
}

/// One connection, the root window and the interned atoms, shared by one lookup.
struct Scan {
    conn: RustConnection,
    root: Window,
    atoms: Atoms,
}

impl Scan {
    fn open() -> Result<Self, PortError> {
        let (conn, screen_num) = x11rb::connect(None).map_err(fail)?;
        let root = conn.setup().roots[screen_num].root;
        let atoms = Atoms::intern(&conn).map_err(fail)?;
        Ok(Self { conn, root, atoms })
    }

    /// `_NET_CLIENT_LIST_STACKING` (bottom to top), else `_NET_CLIENT_LIST`. None when neither is set,
    /// which means no EWMH window manager is running.
    fn root_windows(&self) -> Result<Option<Vec<Window>>, PortError> {
        for atom in [self.atoms.client_list_stacking, self.atoms.client_list] {
            if let Some(ids) = self.u32s(self.root, atom).map_err(fail)? {
                return Ok(Some(ids));
            }
        }
        Ok(None)
    }

    /// The hover target for `win`, or None when it is not something to snap to: unmapped, hidden,
    /// or 1x1 or smaller. An X error means the window is gone, and the caller decides what that means.
    fn target(&self, win: Window) -> Result<Option<HoverTarget>, ReplyError> {
        let attrs = self.conn.get_window_attributes(win)?.reply()?;
        if attrs.map_state != MapState::VIEWABLE {
            return Ok(None);
        }
        if self.u32s(win, self.atoms.wm_state)?.unwrap_or_default().contains(&self.atoms.wm_state_hidden) {
            return Ok(None);
        }
        let geometry = self.conn.get_geometry(win)?.reply()?;
        if is_tiny(u32::from(geometry.width), u32::from(geometry.height)) {
            return Ok(None);
        }
        let origin = self.conn.translate_coordinates(win, self.root, 0, 0)?.reply()?;
        let extents = self.u32s(win, self.atoms.frame_extents)?.as_deref().and_then(parse_frame_extents).unwrap_or_default();
        let client = PhysRect {
            x: i32::from(origin.dst_x),
            y: i32::from(origin.dst_y),
            width: u32::from(geometry.width),
            height: u32::from(geometry.height),
        };
        Ok(Some(HoverTarget { rect: expand(client, extents), title: self.title(win)?, process: self.process(win)?, kind: TargetKind::Window }))
    }

    /// `_NET_WM_NAME` (UTF-8), else `WM_NAME`. Empty when neither is set.
    fn title(&self, win: Window) -> Result<String, ReplyError> {
        let utf8 = self.bytes(win, self.atoms.net_wm_name, self.atoms.utf8_string)?.filter(|b| !b.is_empty());
        let bytes = match utf8 {
            Some(bytes) => bytes,
            None => self.bytes(win, AtomEnum::WM_NAME, AtomEnum::ANY)?.unwrap_or_default(),
        };
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// The name of the window's process: `/proc/<pid>/comm` when `_NET_WM_PID` gives a readable one,
    /// else the class half of `WM_CLASS`. Empty when neither is available.
    fn process(&self, win: Window) -> Result<String, ReplyError> {
        let from_pid = self
            .u32s(win, self.atoms.net_wm_pid)?
            .and_then(|pids| pids.first().copied())
            .and_then(|pid| fs::read_to_string(format!("/proc/{pid}/comm")).ok())
            .map(|comm| parse_comm(&comm))
            .filter(|name| !name.is_empty());
        if let Some(name) = from_pid {
            return Ok(name);
        }
        let class = self.bytes(win, AtomEnum::WM_CLASS, AtomEnum::ANY)?.unwrap_or_default();
        Ok(parse_wm_class(&class).1)
    }

    /// A property's 32-bit values, or None when it is missing or has another format.
    fn u32s(&self, win: Window, prop: Atom) -> Result<Option<Vec<u32>>, ReplyError> {
        let reply = self.conn.get_property(false, win, prop, AtomEnum::ANY, 0, LONG_LEN)?.reply()?;
        Ok(reply.value32().map(|values| values.collect::<Vec<u32>>()))
    }

    /// A property's bytes when it has 8-bit format, or None otherwise.
    fn bytes(&self, win: Window, prop: impl Into<Atom>, ty: impl Into<Atom>) -> Result<Option<Vec<u8>>, ReplyError> {
        let reply = self.conn.get_property(false, win, prop, ty, 0, LONG_LEN)?.reply()?;
        Ok((reply.format == 8).then_some(reply.value))
    }
}

/// The atoms the lookups read, interned once per connection.
struct Atoms {
    client_list_stacking: Atom,
    client_list: Atom,
    wm_state: Atom,
    wm_state_hidden: Atom,
    frame_extents: Atom,
    net_wm_name: Atom,
    utf8_string: Atom,
    net_wm_pid: Atom,
    active_window: Atom,
}

impl Atoms {
    fn intern(conn: &RustConnection) -> Result<Self, ReplyError> {
        let names: [&[u8]; 9] = [
            b"_NET_CLIENT_LIST_STACKING",
            b"_NET_CLIENT_LIST",
            b"_NET_WM_STATE",
            b"_NET_WM_STATE_HIDDEN",
            b"_NET_FRAME_EXTENTS",
            b"_NET_WM_NAME",
            b"UTF8_STRING",
            b"_NET_WM_PID",
            b"_NET_ACTIVE_WINDOW",
        ];
        let [client_list_stacking, client_list, wm_state, wm_state_hidden, frame_extents, net_wm_name, utf8_string, net_wm_pid, active_window] =
            intern_all(conn, names)?;
        Ok(Self {
            client_list_stacking,
            client_list,
            wm_state,
            wm_state_hidden,
            frame_extents,
            net_wm_name,
            utf8_string,
            net_wm_pid,
            active_window,
        })
    }
}

/// Sends every `InternAtom` before reading any reply, so the round trips overlap.
fn intern_all<const N: usize>(conn: &RustConnection, names: [&[u8]; N]) -> Result<[Atom; N], ReplyError> {
    let cookies = names.map(|name| conn.intern_atom(false, name));
    let mut atoms = [0; N];
    for (slot, cookie) in atoms.iter_mut().zip(cookies) {
        *slot = cookie?.reply()?.atom;
    }
    Ok(atoms)
}

/// The window list is bottom to top, and hover wants front to back.
fn front_to_back(mut ids: Vec<Window>) -> Vec<Window> {
    ids.reverse();
    ids
}

/// `WM_CLASS` is `instance\0class\0`. Returns `(instance, class)`, with an absent part left empty.
pub(crate) fn parse_wm_class(bytes: &[u8]) -> (String, String) {
    let mut parts = bytes.split(|b| *b == 0).map(|part| String::from_utf8_lossy(part).into_owned());
    let instance = parts.next().unwrap_or_default();
    let class = parts.next().unwrap_or_default();
    (instance, class)
}

/// `_NET_FRAME_EXTENTS` is `left, right, top, bottom`. Any other length is malformed.
pub(crate) fn parse_frame_extents(values: &[u32]) -> Option<(u32, u32, u32, u32)> {
    match values {
        [left, right, top, bottom] => Some((*left, *right, *top, *bottom)),
        _ => None,
    }
}

/// Grows a client rect to the frame the window manager draws around it.
pub(crate) fn expand(rect: PhysRect, (left, right, top, bottom): (u32, u32, u32, u32)) -> PhysRect {
    PhysRect {
        x: rect.x.saturating_sub_unsigned(left),
        y: rect.y.saturating_sub_unsigned(top),
        width: rect.width.saturating_add(left).saturating_add(right),
        height: rect.height.saturating_add(top).saturating_add(bottom),
    }
}

/// A window of 1x1 or smaller is not offered as a target.
pub(crate) fn is_tiny(width: u32, height: u32) -> bool {
    width <= 1 && height <= 1
}

/// `/proc/<pid>/comm` holds the process name with a trailing newline.
pub(crate) fn parse_comm(contents: &str) -> String {
    contents.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use laqta_domain::PhysRect;

    use super::{expand, front_to_back, is_tiny, parse_comm, parse_frame_extents, parse_wm_class};

    #[test]
    fn wm_class_splits_instance_and_class() {
        assert_eq!(parse_wm_class(b"firefox\0Firefox\0"), ("firefox".to_owned(), "Firefox".to_owned()));
    }

    #[test]
    fn wm_class_without_trailing_nul_still_splits() {
        assert_eq!(parse_wm_class(b"code\0Code"), ("code".to_owned(), "Code".to_owned()));
    }

    #[test]
    fn wm_class_with_one_string_has_an_empty_class() {
        assert_eq!(parse_wm_class(b"only\0"), ("only".to_owned(), String::new()));
        assert_eq!(parse_wm_class(b"only"), ("only".to_owned(), String::new()));
    }

    #[test]
    fn wm_class_empty_bytes_gives_two_empty_strings() {
        assert_eq!(parse_wm_class(b""), (String::new(), String::new()));
    }

    #[test]
    fn frame_extents_need_exactly_four_values_in_order() {
        assert_eq!(parse_frame_extents(&[1, 2, 3, 4]), Some((1, 2, 3, 4)));
        assert_eq!(parse_frame_extents(&[1, 2, 3]), None);
        assert_eq!(parse_frame_extents(&[]), None);
        assert_eq!(parse_frame_extents(&[1, 2, 3, 4, 5]), None);
    }

    #[test]
    fn expand_grows_by_left_right_top_bottom() {
        let client = PhysRect { x: 100, y: 50, width: 200, height: 100 };
        assert_eq!(expand(client, (4, 4, 30, 4)), PhysRect { x: 96, y: 20, width: 208, height: 134 });
    }

    #[test]
    fn expand_with_no_extents_is_the_client_rect() {
        let client = PhysRect { x: -10, y: 5, width: 7, height: 9 };
        assert_eq!(expand(client, (0, 0, 0, 0)), client);
    }

    #[test]
    fn expand_saturates_at_the_coordinate_limits() {
        let client = PhysRect { x: i32::MIN, y: i32::MIN, width: u32::MAX, height: u32::MAX };
        let grown = expand(client, (10, 10, 10, 10));
        assert_eq!((grown.x, grown.y), (i32::MIN, i32::MIN));
        assert_eq!((grown.width, grown.height), (u32::MAX, u32::MAX));
    }

    #[test]
    fn stacking_list_is_reversed_to_front_to_back() {
        assert_eq!(front_to_back(vec![1, 2, 3]), vec![3, 2, 1]);
        assert_eq!(front_to_back(Vec::new()), Vec::<u32>::new());
    }

    #[test]
    fn one_by_one_or_smaller_is_tiny() {
        assert!(is_tiny(1, 1));
        assert!(is_tiny(0, 0));
        assert!(!is_tiny(0, 5));
        assert!(!is_tiny(2, 1));
        assert!(!is_tiny(1, 2));
        assert!(!is_tiny(640, 480));
    }

    #[test]
    fn comm_drops_the_trailing_newline() {
        assert_eq!(parse_comm("firefox\n"), "firefox");
        assert_eq!(parse_comm(" \n"), "");
    }
}
