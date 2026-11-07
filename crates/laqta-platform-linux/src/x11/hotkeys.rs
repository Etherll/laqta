use std::sync::Mutex;

use laqta_domain::{Action, Key, KeyChord, Modifiers};
use laqta_ports::{ActionCallback, HotkeyRegistrar, PortError};
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{ConnectionExt as _, GrabMode, ModMask};

const XK_PRINT: u32 = 0xff61;

/// `XGrabKey` on the root window, on its own connection and thread.
#[derive(Default)]
pub struct X11Hotkeys {
    started: Mutex<bool>,
}

fn keysym(key: Key) -> u32 {
    match key {
        Key::PrintScreen => XK_PRINT,
        #[allow(unreachable_patterns)]
        _ => 0,
    }
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

impl HotkeyRegistrar for X11Hotkeys {
    fn register(&self, bindings: &[(KeyChord, Action)], on_fire: ActionCallback) -> Vec<(KeyChord, Result<(), PortError>)> {
        let all_err = |e: PortError| bindings.iter().map(|(c, _)| (*c, Err(e.clone()))).collect::<Vec<_>>();
        if let Ok(mut s) = self.started.lock() {
            if *s {
                return all_err(PortError::Failed("hotkeys already registered".into()));
            }
            *s = true;
        }
        let (conn, screen_num) = match x11rb::connect(None) {
            Ok(c) => c,
            Err(e) => return all_err(PortError::Failed(format!("x11: {e}"))),
        };
        let root = conn.setup().roots[screen_num].root;
        let (min, max) = (conn.setup().min_keycode, conn.setup().max_keycode);
        let mapping = match conn.get_keyboard_mapping(min, max - min + 1).map(|c| c.reply()) {
            Ok(Ok(m)) => m,
            _ => return all_err(PortError::Failed("x11: keyboard mapping".into())),
        };
        let per = mapping.keysyms_per_keycode as usize;
        let keycode_of = |sym: u32| -> Option<u8> {
            mapping.keysyms.chunks(per.max(1)).position(|ks| ks.contains(&sym)).map(|i| min + i as u8)
        };
        let mut grabbed: Vec<(u8, ModMask, Action)> = Vec::new();
        let mut results = Vec::new();
        for (chord, action) in bindings {
            let Some(code) = keycode_of(keysym(chord.key)) else {
                results.push((*chord, Err(PortError::Unsupported(format!("no keycode for {:?}", chord.key)))));
                continue;
            };
            let base = mod_mask(chord.modifiers);
            let mut ok = true;
            // Also grab with NumLock (Mod2) and CapsLock (Lock) so the hotkey works with them on.
            for extra in [ModMask::from(0u16), ModMask::M2, ModMask::LOCK, ModMask::M2 | ModMask::LOCK] {
                let r = conn.grab_key(true, root, base | extra, code, GrabMode::ASYNC, GrabMode::ASYNC).map(|c| c.check());
                ok &= matches!(r, Ok(Ok(())));
            }
            if ok {
                grabbed.push((code, base, *action));
                results.push((*chord, Ok(())));
            } else {
                results.push((*chord, Err(PortError::Failed("key already grabbed by another client".into()))));
            }
        }
        let _ = conn.flush();
        if !grabbed.is_empty() {
            let spawned = std::thread::Builder::new().name("laqta-x11-hotkeys".into()).spawn(move || {
                let ignore = u16::from(ModMask::M2 | ModMask::LOCK);
                while let Ok(event) = conn.wait_for_event() {
                    if let Event::KeyPress(k) = event {
                        let state = u16::from(k.state) & !ignore & 0xff;
                        if let Some((_, _, action)) = grabbed.iter().find(|(c, m, _)| *c == k.detail && u16::from(*m) == state) {
                            on_fire(*action);
                        }
                    }
                }
            });
            if let Err(e) = spawned {
                return all_err(PortError::Failed(e.to_string()));
            }
        }
        results
    }
}
