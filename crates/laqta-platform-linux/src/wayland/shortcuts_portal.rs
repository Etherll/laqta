use std::sync::{Mutex, mpsc};
use std::thread;
use std::time::Duration;

use ashpd::desktop::global_shortcuts::{BindShortcutsOptions, ConfigureShortcutsOptions, GlobalShortcuts, NewShortcut, Shortcut};
use ashpd::desktop::{CreateSessionOptions, Session};
use futures_lite::{StreamExt, future};
use laqta_domain::{Action, Key, KeyChord, Modifiers};
use laqta_ports::{AssignedCallback, HotkeyCallback, HotkeyRegistrar, PortError};

/// GlobalShortcuts portal: one session at a time. Shortcut ids come from the action's name
/// (`PrintScreen` → `print-screen`). A missing portal is `Unsupported`.
#[derive(Default)]
pub struct PortalShortcuts {
    /// Sends work to the listener's session. `Some` while a registration is live; dropping the
    /// sender (or sending `Control::Stop`) ends the session.
    control: Mutex<Option<mpsc::Sender<Control>>>,
    /// Set by `on_assigned`, before `register` starts the listener.
    assigned: Mutex<Option<AssignedCallback>>,
}

/// Work for the listener's session, sent from the caller's thread.
enum Control {
    /// Ends the session.
    Stop,
    /// Opens the desktop's shortcut settings and sends the result to the reply channel.
    Configure(mpsc::Sender<Result<(), PortError>>),
}

impl PortalShortcuts {
    fn take_control(&self) -> Option<mpsc::Sender<Control>> {
        self.control.lock().ok().and_then(|mut control| control.take())
    }
}

impl HotkeyRegistrar for PortalShortcuts {
    fn register(&self, bindings: &[(KeyChord, Action)], on_fire: HotkeyCallback) -> Vec<(KeyChord, Result<(), PortError>)> {
        let all = |r: Result<(), PortError>| bindings.iter().map(|(c, _)| (*c, r.clone())).collect::<Vec<_>>();
        let (ready_tx, ready_rx) = mpsc::channel();
        {
            let Ok(mut control) = self.control.lock() else {
                return all(Err(PortError::Failed("hotkey state is poisoned".into())));
            };
            if control.is_some() {
                return all(Err(PortError::Failed("hotkeys already registered".into())));
            }
            let (control_tx, control_rx) = mpsc::channel();
            let on_assigned = self.assigned.lock().ok().and_then(|assigned| assigned.clone());
            let bindings = bindings.to_vec();
            let spawned = thread::Builder::new().name("laqta-shortcuts".into()).spawn(move || {
                async_io::block_on(listen(bindings, on_fire, on_assigned, ready_tx, control_rx));
            });
            if let Err(e) = spawned {
                return all(Err(PortError::Failed(e.to_string())));
            }
            *control = Some(control_tx);
        }
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => all(Ok(())),
            Ok(Err(e)) => {
                // The listener has already exited, so nothing needs stopping: allow a retry.
                drop(self.take_control());
                all(Err(e))
            }
            Err(_) => all(Err(PortError::Unsupported("GlobalShortcuts portal did not answer within 5 s".into()))),
        }
    }

    fn unregister_all(&self) {
        if let Some(tx) = self.take_control() {
            let _ = tx.send(Control::Stop);
        }
    }

    fn on_assigned(&self, assigned: AssignedCallback) {
        if let Ok(mut slot) = self.assigned.lock() {
            *slot = Some(assigned);
        }
    }

    fn configure(&self) -> Result<(), PortError> {
        let control = self.control.lock().map_err(|_| PortError::Failed("hotkey state is poisoned".into()))?.clone();
        let Some(control) = control else {
            return Err(PortError::Unsupported("no shortcut session is running".into()));
        };
        let (reply_tx, reply_rx) = mpsc::channel();
        if control.send(Control::Configure(reply_tx)).is_err() {
            return Err(PortError::Unsupported("no shortcut session is running".into()));
        }
        match reply_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => Err(PortError::Unsupported("GlobalShortcuts portal did not answer within 5 s".into())),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(PortError::Unsupported("no shortcut session is running".into())),
        }
    }
}

/// One GlobalShortcuts session: binds the shortcuts and forwards activations until a stop, then
/// closes the session. `ready` reports whether the portal exists and accepted the session.
async fn listen(
    bindings: Vec<(KeyChord, Action)>,
    on_fire: HotkeyCallback,
    on_assigned: Option<AssignedCallback>,
    ready: mpsc::Sender<Result<(), PortError>>,
    control: mpsc::Receiver<Control>,
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
    let ids = binding_ids(&bindings);
    let shortcuts: Vec<NewShortcut> = bindings
        .iter()
        .zip(&ids)
        .map(|((c, a), id)| NewShortcut::new(id.as_str(), a.label()).preferred_trigger(trigger(*c).as_deref()))
        .collect();
    // Ending the control loop (a stop) cancels the serving future, so it also interrupts a pending bind dialog.
    future::or(
        serve(&portal, &session, &shortcuts, &bindings, &ids, &on_fire, on_assigned.as_ref()),
        control_loop(&portal, &session, control),
    )
    .await;
    if let Err(e) = session.close().await {
        tracing::warn!("GlobalShortcuts close failed: {e}");
    }
}

/// Binds the shortcuts, then forwards each activation to `on_fire`, and each change of the keys
/// the desktop assigned to `on_assigned`, until both streams end.
async fn serve(
    portal: &GlobalShortcuts,
    session: &Session<GlobalShortcuts>,
    shortcuts: &[NewShortcut],
    bindings: &[(KeyChord, Action)],
    ids: &[String],
    on_fire: &HotkeyCallback,
    on_assigned: Option<&AssignedCallback>,
) {
    // Subscribe before binding, so an activation or a key change that lands right after the bind
    // isn't missed.
    let activated = portal.receive_activated().await;
    let changed = match on_assigned {
        Some(_) => Some(portal.receive_shortcuts_changed().await),
        None => None,
    };
    match portal.bind_shortcuts(session, shortcuts, None, BindShortcutsOptions::default()).await.map(|r| r.response()) {
        Ok(Ok(response)) => {
            tracing::info!("GlobalShortcuts bound");
            if let Some(on_assigned) = on_assigned {
                on_assigned(assigned(ids, bindings, pairs(response.shortcuts())));
            }
        }
        Ok(Err(e)) | Err(e) => tracing::warn!("GlobalShortcuts bind failed: {e}"),
    }
    let activations = async {
        let Ok(mut stream) = activated else {
            tracing::warn!("GlobalShortcuts: no activation stream");
            return;
        };
        while let Some(activated) = stream.next().await {
            if let Some(index) = ids.iter().position(|id| id == activated.shortcut_id()) {
                on_fire(bindings[index].0);
            }
        }
    };
    let changes = async {
        let (Some(on_assigned), Some(changed)) = (on_assigned, changed) else { return };
        let Ok(mut stream) = changed else {
            tracing::warn!("GlobalShortcuts: no shortcut-change stream");
            return;
        };
        while let Some(signal) = stream.next().await {
            let keys = changed_keys(ids, bindings, pairs(signal.shortcuts()));
            if !keys.is_empty() {
                on_assigned(keys);
            }
        }
    };
    future::zip(activations, changes).await;
}

/// Serves the listener's control messages: opens the desktop's shortcut settings on `Configure`,
/// and returns on `Stop` or once every sender is gone.
async fn control_loop(portal: &GlobalShortcuts, session: &Session<GlobalShortcuts>, mut inbox: mpsc::Receiver<Control>) {
    loop {
        // The blocking receive runs off the executor; the receiver comes back with the message.
        let (next, back) = blocking::unblock(move || {
            let next = inbox.recv();
            (next, inbox)
        })
        .await;
        inbox = back;
        match next {
            Ok(Control::Configure(reply)) => {
                // Bounded, so a portal that never answers can't keep a Stop from being read.
                let configure = async {
                    portal
                        .configure_shortcuts(session, None, ConfigureShortcutsOptions::default())
                        .await
                        .map_err(|e| PortError::Failed(format!("GlobalShortcuts configure: {e}")))
                };
                let timeout = async {
                    async_io::Timer::after(Duration::from_secs(5)).await;
                    Err(PortError::Unsupported("GlobalShortcuts portal did not answer within 5 s".into()))
                };
                let _ = reply.send(future::or(configure, timeout).await);
            }
            Ok(Control::Stop) | Err(_) => return,
        }
    }
}

/// The `(id, key text)` pairs of a portal response's shortcuts.
fn pairs(shortcuts: &[Shortcut]) -> Vec<(String, String)> {
    shortcuts.iter().map(|s| (s.id().to_owned(), s.trigger_description().to_owned())).collect()
}

/// Each binding's key from the portal's list: None for a binding the list leaves out. Ids that
/// match no binding are dropped.
fn listed_keys(
    ids: &[String],
    bindings: &[(KeyChord, Action)],
    shortcuts: impl IntoIterator<Item = (String, String)>,
) -> Vec<(KeyChord, Option<String>)> {
    let mut keys = vec![None; bindings.len()];
    for (id, key) in shortcuts {
        if let Some(index) = ids.iter().position(|known| *known == id) {
            keys[index] = Some(key);
        }
    }
    bindings.iter().zip(keys).map(|((chord, _), key)| (*chord, key)).collect()
}

/// The keys from a bind response: every binding, with "" where the desktop gave none.
fn assigned(
    ids: &[String],
    bindings: &[(KeyChord, Action)],
    shortcuts: impl IntoIterator<Item = (String, String)>,
) -> Vec<(KeyChord, String)> {
    listed_keys(ids, bindings, shortcuts).into_iter().map(|(chord, key)| (chord, key.unwrap_or_default())).collect()
}

/// The keys from a changed signal: only the shortcuts it lists.
fn changed_keys(
    ids: &[String],
    bindings: &[(KeyChord, Action)],
    shortcuts: impl IntoIterator<Item = (String, String)>,
) -> Vec<(KeyChord, String)> {
    listed_keys(ids, bindings, shortcuts).into_iter().filter_map(|(chord, key)| key.map(|key| (chord, key))).collect()
}

/// One portal id per binding, in order. The first binding of an action keeps the action's plain id,
/// so triggers the user already set in the desktop's settings still apply. A later binding of the
/// same action (another workflow on another chord) gets `-2`, `-3`, …, so its activation is told
/// apart from the first.
fn binding_ids(bindings: &[(KeyChord, Action)]) -> Vec<String> {
    let mut ids = Vec::with_capacity(bindings.len());
    for (index, (_, action)) in bindings.iter().enumerate() {
        let earlier = bindings[..index].iter().filter(|(_, a)| a == action).count();
        let id = shortcut_id(*action);
        ids.push(if earlier == 0 { id } else { format!("{id}-{}", earlier + 1) });
    }
    ids
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
        Key::VolumeUp => "XF86AudioRaiseVolume",
        Key::VolumeDown => "XF86AudioLowerVolume",
        Key::VolumeMute => "XF86AudioMute",
        Key::MediaPlayPause => "XF86AudioPlay",
        Key::MediaNext => "XF86AudioNext",
        Key::MediaPrev => "XF86AudioPrev",
        Key::MediaStop => "XF86AudioStop",
        Key::LaunchMail => "XF86Mail",
        // As on X11: Windows' launch keys are My Computer and Calculator.
        Key::LaunchApp1 => "XF86MyComputer",
        Key::LaunchApp2 | Key::Calculator => "XF86Calculator",
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

    /// Two workflows on the same action each get their own portal id, and the first keeps the
    /// plain one.
    #[test]
    fn same_action_bindings_get_distinct_ids() {
        use laqta_domain::{Key, KeyChord, Modifiers};
        let chord = |key| KeyChord { modifiers: Modifiers::CTRL, key };
        let bindings = [
            (chord(Key::F1), Action::RectangleRegion),
            (chord(Key::F2), Action::PrintScreen),
            (chord(Key::F3), Action::RectangleRegion),
            (chord(Key::F4), Action::RectangleRegion),
        ];
        assert_eq!(binding_ids(&bindings), ["rectangle-region", "print-screen", "rectangle-region-2", "rectangle-region-3"]);
    }

    /// A bind response gives every binding a key: the ones it lists, and "" for the rest. Ids
    /// that match no binding are dropped.
    #[test]
    fn assigned_covers_every_binding_and_drops_unknown_ids() {
        let bindings = [
            (chord("Ctrl+1"), Action::PrintScreen),
            (chord("Ctrl+2"), Action::RectangleRegion),
            (chord("Ctrl+3"), Action::RectangleRegion),
        ];
        let ids = binding_ids(&bindings);
        let reported = assigned(
            &ids,
            &bindings,
            [("rectangle-region-2".to_owned(), "Meta+Shift+S".to_owned()), ("unknown".to_owned(), "Meta+X".to_owned())],
        );
        assert_eq!(
            reported,
            [(chord("Ctrl+1"), String::new()), (chord("Ctrl+2"), String::new()), (chord("Ctrl+3"), "Meta+Shift+S".to_owned())]
        );
    }

    /// A changed signal reports only the shortcuts it lists.
    #[test]
    fn changed_keys_report_only_listed_shortcuts() {
        let bindings = [(chord("Ctrl+1"), Action::PrintScreen), (chord("Ctrl+2"), Action::RectangleRegion)];
        let ids = binding_ids(&bindings);
        assert!(changed_keys(&ids, &bindings, Vec::<(String, String)>::new()).is_empty());
        let reported = changed_keys(&ids, &bindings, [("rectangle-region".to_owned(), "Print".to_owned())]);
        assert_eq!(reported, [(chord("Ctrl+2"), "Print".to_owned())]);
    }
}
