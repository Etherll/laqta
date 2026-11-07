use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use laqta_domain::{Action, Key, KeyChord, Modifiers};
use laqta_ports::{ActionCallback, HotkeyRegistrar, PortError};
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{ConnectionExt as _, GrabMode, ModMask, Window};
use x11rb::rust_connection::RustConnection;

use super::fail;

/// `XGrabKey` on the root window. The first `register` opens a connection and starts the event
/// thread. Later calls add grabs to that connection, and `unregister_all` drops them all.
#[derive(Default)]
pub struct X11Hotkeys {
    session: Mutex<Option<Session>>,
}

/// The connection that owns the grabs. X releases a grab only through the connection that made it,
/// so `register` and `unregister_all` both send on this one, and the event thread waits on it.
struct Session {
    conn: Arc<RustConnection>,
    root: Window,
    grabs: Arc<Mutex<Vec<Grab>>>,
}

/// One grabbed chord. Each binding keeps the callback of the `register` call that added it.
struct Grab {
    keycode: u8,
    /// The chord's modifiers, without the NumLock and CapsLock bits.
    modifiers: ModMask,
    action: Action,
    on_fire: ActionCallback,
}

impl Session {
    fn open() -> Result<Self, PortError> {
        let (conn, screen_num) = x11rb::connect(None).map_err(fail)?;
        let root = conn.setup().roots[screen_num].root;
        let conn = Arc::new(conn);
        let grabs = Arc::new(Mutex::new(Vec::new()));
        let (thread_conn, thread_grabs) = (Arc::clone(&conn), Arc::clone(&grabs));
        std::thread::Builder::new()
            .name("laqta-x11-hotkeys".into())
            .spawn(move || event_loop(&thread_conn, &thread_grabs))
            .map_err(fail)?;
        Ok(Self { conn, root, grabs })
    }
}

/// Runs until the connection drops. The binding is looked up under the lock, but the callback runs
/// after the lock is released, so it may call `register` or `unregister_all`.
fn event_loop(conn: &RustConnection, grabs: &Mutex<Vec<Grab>>) {
    let ignored = u16::from(ModMask::M2 | ModMask::LOCK);
    while let Ok(event) = conn.wait_for_event() {
        let Event::KeyPress(press) = event else {
            continue;
        };
        let state = u16::from(press.state) & !ignored & 0xff;
        let fired = lock(grabs)
            .iter()
            .find(|g| g.keycode == press.detail && u16::from(g.modifiers) == state)
            .map(|g| (g.action, Arc::clone(&g.on_fire)));
        if let Some((action, on_fire)) = fired {
            on_fire(action);
        }
    }
}

/// X keysyms for the keys in `Key`, in runs that map to consecutive keysyms.
const LETTERS: [Key; 26] = [
    Key::A,
    Key::B,
    Key::C,
    Key::D,
    Key::E,
    Key::F,
    Key::G,
    Key::H,
    Key::I,
    Key::J,
    Key::K,
    Key::L,
    Key::M,
    Key::N,
    Key::O,
    Key::P,
    Key::Q,
    Key::R,
    Key::S,
    Key::T,
    Key::U,
    Key::V,
    Key::W,
    Key::X,
    Key::Y,
    Key::Z,
];
const DIGITS: [Key; 10] =
    [Key::Digit0, Key::Digit1, Key::Digit2, Key::Digit3, Key::Digit4, Key::Digit5, Key::Digit6, Key::Digit7, Key::Digit8, Key::Digit9];
const FUNCTION_KEYS: [Key; 24] = [
    Key::F1,
    Key::F2,
    Key::F3,
    Key::F4,
    Key::F5,
    Key::F6,
    Key::F7,
    Key::F8,
    Key::F9,
    Key::F10,
    Key::F11,
    Key::F12,
    Key::F13,
    Key::F14,
    Key::F15,
    Key::F16,
    Key::F17,
    Key::F18,
    Key::F19,
    Key::F20,
    Key::F21,
    Key::F22,
    Key::F23,
    Key::F24,
];
const KEYPAD_DIGITS: [Key; 10] = [
    Key::Numpad0,
    Key::Numpad1,
    Key::Numpad2,
    Key::Numpad3,
    Key::Numpad4,
    Key::Numpad5,
    Key::Numpad6,
    Key::Numpad7,
    Key::Numpad8,
    Key::Numpad9,
];

/// The X keysym for a key, or None for a key this adapter does not map.
pub(crate) fn keysym(key: Key) -> Option<u32> {
    let in_run = |run: &[Key], first: u32| run.iter().position(|k| *k == key).map(|i| first + i as u32);
    in_run(&LETTERS, 0x61)
        .or_else(|| in_run(&DIGITS, 0x30))
        .or_else(|| in_run(&FUNCTION_KEYS, 0xffbe))
        .or_else(|| in_run(&KEYPAD_DIGITS, 0xffb0))
        .or(match key {
            Key::PrintScreen => Some(0xff61),
            Key::Pause => Some(0xff13),
            Key::ScrollLock => Some(0xff14),
            Key::Insert => Some(0xff63),
            Key::Delete => Some(0xffff),
            Key::Home => Some(0xff50),
            Key::End => Some(0xff57),
            Key::PageUp => Some(0xff55),
            Key::PageDown => Some(0xff56),
            Key::Left => Some(0xff51),
            Key::Up => Some(0xff52),
            Key::Right => Some(0xff53),
            Key::Down => Some(0xff54),
            Key::Space => Some(0x20),
            Key::Enter => Some(0xff0d),
            Key::Escape => Some(0xff1b),
            Key::Tab => Some(0xff09),
            Key::Backspace => Some(0xff08),
            Key::NumpadAdd => Some(0xffab),
            Key::NumpadSubtract => Some(0xffad),
            Key::NumpadMultiply => Some(0xffaa),
            Key::NumpadDivide => Some(0xffaf),
            Key::NumpadDecimal => Some(0xffae),
            Key::Minus => Some(0x2d),
            Key::Equal => Some(0x3d),
            Key::Comma => Some(0x2c),
            Key::Period => Some(0x2e),
            Key::Slash => Some(0x2f),
            Key::Backslash => Some(0x5c),
            Key::Semicolon => Some(0x3b),
            Key::Quote => Some(0x27),
            Key::BracketLeft => Some(0x5b),
            Key::BracketRight => Some(0x5d),
            Key::Backquote => Some(0x60),
            _ => None,
        })
}

fn mod_mask(m: Modifiers) -> ModMask {
    let mut mask = ModMask::from(0u16);
    if m.contains(Modifiers::SHIFT) {
        mask |= ModMask::SHIFT;
    }
    if m.contains(Modifiers::CTRL) {
        mask |= ModMask::CONTROL;
    }
    if m.contains(Modifiers::ALT) {
        mask |= ModMask::M1;
    }
    if m.contains(Modifiers::SUPER) {
        mask |= ModMask::M4;
    }
    mask
}

/// The NumLock (Mod2) and CapsLock (Lock) combinations of a chord's modifiers.
fn lock_variants() -> [ModMask; 4] {
    [ModMask::from(0u16), ModMask::M2, ModMask::LOCK, ModMask::M2 | ModMask::LOCK]
}

/// Grabs the chord with and without NumLock and CapsLock, so it works with either on. If any
/// variant fails, the ones that succeeded are released, so no key is swallowed with no binding.
fn grab_chord(conn: &RustConnection, root: Window, keycode: u8, modifiers: ModMask) -> bool {
    let mut all = true;
    for extra in lock_variants() {
        let grabbed = conn.grab_key(true, root, modifiers | extra, keycode, GrabMode::ASYNC, GrabMode::ASYNC).map(|c| c.check());
        all &= matches!(grabbed, Ok(Ok(())));
    }
    if !all {
        for extra in lock_variants() {
            let _ = conn.ungrab_key(keycode, root, modifiers | extra);
        }
    }
    all
}

/// Locks, recovering the data from a poisoned mutex. The lists stay usable after a panic elsewhere.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl HotkeyRegistrar for X11Hotkeys {
    fn register(&self, bindings: &[(KeyChord, Action)], on_fire: ActionCallback) -> Vec<(KeyChord, Result<(), PortError>)> {
        let all_err = |e: PortError| bindings.iter().map(|(c, _)| (*c, Err(e.clone()))).collect::<Vec<_>>();
        let mut guard = lock(&self.session);
        if guard.is_none() {
            match Session::open() {
                Ok(opened) => *guard = Some(opened),
                Err(e) => return all_err(e),
            }
        }
        let Some(session) = guard.as_ref() else {
            return all_err(PortError::Failed("x11: no hotkey session".into()));
        };
        let (min, max) = (session.conn.setup().min_keycode, session.conn.setup().max_keycode);
        let mapping = match session.conn.get_keyboard_mapping(min, max - min + 1).map(|c| c.reply()) {
            Ok(Ok(m)) => m,
            _ => return all_err(PortError::Failed("x11: keyboard mapping".into())),
        };
        let per = (mapping.keysyms_per_keycode as usize).max(1);
        let keycode_of = |sym: u32| mapping.keysyms.chunks(per).position(|syms| syms.contains(&sym)).map(|i| min + i as u8);

        let mut results = Vec::with_capacity(bindings.len());
        for (chord, action) in bindings {
            let Some(sym) = keysym(chord.key) else {
                results.push((*chord, Err(PortError::Unsupported(format!("no X keysym for {:?}", chord.key)))));
                continue;
            };
            let Some(keycode) = keycode_of(sym) else {
                results.push((*chord, Err(PortError::Unsupported(format!("no keycode for {:?}", chord.key)))));
                continue;
            };
            let modifiers = mod_mask(chord.modifiers);
            if !grab_chord(&session.conn, session.root, keycode, modifiers) {
                results.push((*chord, Err(PortError::Failed("key already grabbed by another client".into()))));
                continue;
            }
            // A chord registered again replaces its earlier binding, so one key press fires once.
            let mut grabs = lock(&session.grabs);
            grabs.retain(|g| g.keycode != keycode || u16::from(g.modifiers) != u16::from(modifiers));
            grabs.push(Grab { keycode, modifiers, action: *action, on_fire: Arc::clone(&on_fire) });
            results.push((*chord, Ok(())));
        }
        let _ = session.conn.flush();
        results
    }

    fn unregister_all(&self) {
        let guard = lock(&self.session);
        let Some(session) = guard.as_ref() else {
            return;
        };
        lock(&session.grabs).clear();
        // AnyKey with AnyModifier releases every grab this connection holds on the root, including
        // the NumLock and CapsLock variants.
        let _ = session.conn.ungrab_key(0u8, session.root, ModMask::ANY);
        let _ = session.conn.flush();
    }
}

#[cfg(test)]
mod tests {
    use laqta_domain::Key;

    use super::keysym;

    #[test]
    fn keysym_maps_a_sample_of_keys() {
        assert_eq!(keysym(Key::A), Some(0x61));
        assert_eq!(keysym(Key::Z), Some(0x7a));
        assert_eq!(keysym(Key::Digit0), Some(0x30));
        assert_eq!(keysym(Key::Digit9), Some(0x39));
        assert_eq!(keysym(Key::F1), Some(0xffbe));
        assert_eq!(keysym(Key::F24), Some(0xffd5));
        assert_eq!(keysym(Key::PrintScreen), Some(0xff61));
        assert_eq!(keysym(Key::Numpad0), Some(0xffb0));
        assert_eq!(keysym(Key::NumpadAdd), Some(0xffab));
        assert_eq!(keysym(Key::Space), Some(0x20));
        assert_eq!(keysym(Key::Backquote), Some(0x60));
    }

    #[test]
    fn every_key_maps_to_its_own_keysym() {
        let mut syms = Vec::new();
        for key in Key::ALL {
            let sym = keysym(*key);
            assert!(sym.is_some(), "{key:?} has no keysym");
            syms.extend(sym);
        }
        syms.sort_unstable();
        syms.dedup();
        assert_eq!(syms.len(), Key::ALL.len());
    }
}
