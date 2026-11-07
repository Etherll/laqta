use std::sync::{Mutex, mpsc};
use std::thread;
use std::time::Duration;

use ashpd::desktop::global_shortcuts::{BindShortcutsOptions, GlobalShortcuts, NewShortcut};
use ashpd::desktop::{CreateSessionOptions, Session};
use futures_lite::{StreamExt, future};
use laqta_domain::{Action, Key, KeyChord, Modifiers};
use laqta_ports::{ActionCallback, HotkeyRegistrar, PortError};

/// GlobalShortcuts portal: one session at a time. Shortcut ids come from the action's name
/// (`PrintScreen` → `print-screen`). A missing portal is `Unsupported`.
#[derive(Default)]
pub struct PortalShortcuts {
    /// Ends the listener's session when sent to (or dropped). `Some` while a registration is live.
    stop: Mutex<Option<mpsc::Sender<()>>>,
}

impl PortalShortcuts {
    fn take_stop(&self) -> Option<mpsc::Sender<()>> {
        self.stop.lock().ok().and_then(|mut stop| stop.take())
    }
}

impl HotkeyRegistrar for PortalShortcuts {
    fn register(&self, bindings: &[(KeyChord, Action)], on_fire: ActionCallback) -> Vec<(KeyChord, Result<(), PortError>)> {
        let all = |r: Result<(), PortError>| bindings.iter().map(|(c, _)| (*c, r.clone())).collect::<Vec<_>>();
        let (ready_tx, ready_rx) = mpsc::channel();
        {
            let Ok(mut stop) = self.stop.lock() else {
                return all(Err(PortError::Failed("hotkey state is poisoned".into())));
            };
            if stop.is_some() {
                return all(Err(PortError::Failed("hotkeys already registered".into())));
            }
            let (stop_tx, stop_rx) = mpsc::channel();
            let bindings = bindings.to_vec();
            let spawned = thread::Builder::new().name("laqta-shortcuts".into()).spawn(move || {
                async_io::block_on(listen(bindings, on_fire, ready_tx, stop_rx));
            });
            if let Err(e) = spawned {
                return all(Err(PortError::Failed(e.to_string())));
            }
            *stop = Some(stop_tx);
        }
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => all(Ok(())),
            Ok(Err(e)) => {
                // The listener has already exited, so nothing needs stopping: allow a retry.
                drop(self.take_stop());
                all(Err(e))
            }
            Err(_) => all(Err(PortError::Unsupported("GlobalShortcuts portal did not answer within 5 s".into()))),
        }
    }

    fn unregister_all(&self) {
        if let Some(tx) = self.take_stop() {
            let _ = tx.send(());
        }
    }
}

/// One GlobalShortcuts session: binds the shortcuts and forwards activations until `stop` fires,
/// then closes the session. `ready` reports whether the portal exists and accepted the session.
async fn listen(
    bindings: Vec<(KeyChord, Action)>,
    on_fire: ActionCallback,
    ready: mpsc::Sender<Result<(), PortError>>,
    stop: mpsc::Receiver<()>,
) {
    let portal = match GlobalShortcuts::new().await {
        Ok(p) => p,
        Err(e) => {
            let _ = ready.send(Err(PortError::Unsupported(format!("GlobalShortcuts portal: {e}"))));
            return;
        }
    };
    let session = match portal.create_session(CreateSessionOptions::default()).await {
        Ok(s) => s,
        Err(e) => {
            let _ = ready.send(Err(PortError::Unsupported(format!("GlobalShortcuts session: {e}"))));
            return;
        }
    };
    // The portal is present: report success now; binding may show a dialog.
    let _ = ready.send(Ok(()));
    let shortcuts: Vec<NewShortcut> =
        bindings.iter().map(|(c, a)| NewShortcut::new(shortcut_id(*a), a.label()).preferred_trigger(trigger(*c).as_deref())).collect();
    // Dropping the wait cancels the serving future, so a stop also interrupts a pending bind dialog.
    let stopped = blocking::unblock(move || stop.recv());
    future::or(serve(&portal, &session, &shortcuts, &bindings, &on_fire), async {
        let _ = stopped.await;
    })
    .await;
    if let Err(e) = session.close().await {
        tracing::warn!("GlobalShortcuts close failed: {e}");
    }
}

/// Binds the shortcuts, then forwards each activation to `on_fire` until the stream ends.
async fn serve(
    portal: &GlobalShortcuts,
    session: &Session<GlobalShortcuts>,
    shortcuts: &[NewShortcut],
    bindings: &[(KeyChord, Action)],
    on_fire: &ActionCallback,
) {
    match portal.bind_shortcuts(session, shortcuts, None, BindShortcutsOptions::default()).await.map(|r| r.response()) {
        Ok(Ok(_)) => tracing::info!("GlobalShortcuts bound"),
        Ok(Err(e)) | Err(e) => tracing::warn!("GlobalShortcuts bind failed: {e}"),
    }
    let Ok(mut stream) = portal.receive_activated().await else {
        tracing::warn!("GlobalShortcuts: no activation stream");
        return;
    };
    while let Some(activated) = stream.next().await {
        if let Some((_, action)) = bindings.iter().find(|(_, a)| shortcut_id(*a) == activated.shortcut_id()) {
            on_fire(*action);
        }
    }
}

/// The shortcut id: the variant name in kebab case (`RectangleRegion` → `rectangle-region`).
fn shortcut_id(action: Action) -> String {
    let name = format!("{action:?}");
    let mut id = String::with_capacity(name.len() + 8);
    for (i, c) in name.char_indices() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                id.push('-');
            }
            id.push(c.to_ascii_lowercase());
        } else {
            id.push(c);
        }
    }
    id
}

/// The XDG preferred trigger, e.g. `CTRL+SHIFT+Print`: modifiers in the order CTRL, ALT, SHIFT,
/// LOGO, then the XKB keysym. None when the key has no keysym name.
fn trigger(chord: KeyChord) -> Option<String> {
    let key = keysym(chord.key)?;
    let mut out = String::new();
    for (modifier, name) in [(Modifiers::CTRL, "CTRL"), (Modifiers::ALT, "ALT"), (Modifiers::SHIFT, "SHIFT"), (Modifiers::SUPER, "LOGO")] {
        if chord.modifiers.contains(modifier) {
            out.push_str(name);
            out.push('+');
        }
    }
    out.push_str(&key);
    Some(out)
}

/// The XKB keysym name of a key.
fn keysym(key: Key) -> Option<String> {
    let name = key.name();
    if let [letter] = name.as_bytes()
        && letter.is_ascii_alphabetic()
    {
        return Some(name.to_ascii_lowercase());
    }
    let xkb = match key {
        Key::PrintScreen => "Print",
        Key::Pause => "Pause",
        Key::ScrollLock => "Scroll_Lock",
        Key::Insert => "Insert",
        Key::Delete => "Delete",
        Key::Home => "Home",
        Key::End => "End",
        Key::PageUp => "Prior",
        Key::PageDown => "Next",
        Key::Left => "Left",
        Key::Right => "Right",
        Key::Up => "Up",
        Key::Down => "Down",
        Key::Space => "space",
        Key::Enter => "Return",
        Key::Escape => "Escape",
        Key::Tab => "Tab",
        Key::Backspace => "BackSpace",
        Key::Numpad0 => "KP_0",
        Key::Numpad1 => "KP_1",
        Key::Numpad2 => "KP_2",
        Key::Numpad3 => "KP_3",
        Key::Numpad4 => "KP_4",
        Key::Numpad5 => "KP_5",
        Key::Numpad6 => "KP_6",
        Key::Numpad7 => "KP_7",
        Key::Numpad8 => "KP_8",
        Key::Numpad9 => "KP_9",
        Key::NumpadAdd => "KP_Add",
        Key::NumpadSubtract => "KP_Subtract",
        Key::NumpadMultiply => "KP_Multiply",
        Key::NumpadDivide => "KP_Divide",
        Key::NumpadDecimal => "KP_Decimal",
        Key::Minus => "minus",
        Key::Equal => "equal",
        Key::Comma => "comma",
        Key::Period => "period",
        Key::Slash => "slash",
        Key::Backslash => "backslash",
        Key::Semicolon => "semicolon",
        Key::Quote => "apostrophe",
        Key::BracketLeft => "bracketleft",
        Key::BracketRight => "bracketright",
        Key::Backquote => "grave",
        // Digits and F-keys: their canonical names are the keysym names.
        _ => {
            let plain = name.bytes().all(|b| b.is_ascii_digit()) || (name.starts_with('F') && name.len() <= 3);
            return plain.then(|| name.to_owned());
        }
    };
    Some(xkb.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(text: &str) -> KeyChord {
        text.parse().expect("test chord parses")
    }

    #[test]
    fn ids_are_kebab_case_variant_names() {
        assert_eq!(shortcut_id(Action::PrintScreen), "print-screen");
        assert_eq!(shortcut_id(Action::RectangleRegion), "rectangle-region");
        assert_eq!(shortcut_id(Action::OpenScreenshotsFolder), "open-screenshots-folder");
        assert_eq!(shortcut_id(Action::QrCodeScanRegion), "qr-code-scan-region");
    }

    #[test]
    fn triggers_use_xdg_modifiers_and_keysyms() {
        assert_eq!(trigger(chord("Ctrl+Shift+PrintScreen")).as_deref(), Some("CTRL+SHIFT+Print"));
        assert_eq!(trigger(chord("Alt+Super+A")).as_deref(), Some("ALT+LOGO+a"));
        assert_eq!(trigger(chord("Ctrl+1")).as_deref(), Some("CTRL+1"));
        assert_eq!(trigger(chord("F5")).as_deref(), Some("F5"));
        assert_eq!(trigger(chord("Ctrl+PageUp")).as_deref(), Some("CTRL+Prior"));
        assert_eq!(trigger(chord("Shift+Numpad5")).as_deref(), Some("SHIFT+KP_5"));
        assert_eq!(trigger(chord("Ctrl+Quote")).as_deref(), Some("CTRL+apostrophe"));
    }
}
