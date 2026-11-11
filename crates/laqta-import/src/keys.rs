//! Hotkeys. ShareX stores a `System.Windows.Forms.Keys` value as text, such as `"PrintScreen, Control"`:
//! the key first, then the modifiers, in `, ` or `+` separated form.

use laqta_domain::{Key, KeyChord, Modifiers};

/// The WinForms names that differ from Laqta's `Key::name`. Names that match are found by `key_named`.
const WINFORMS_NAMES: &[(&str, Key)] = &[
    ("Snapshot", Key::PrintScreen),
    ("Return", Key::Enter),
    ("Back", Key::Backspace),
    ("Scroll", Key::ScrollLock),
    ("Prior", Key::PageUp),
    ("Next", Key::PageDown),
    ("Oemplus", Key::Equal),
    ("OemMinus", Key::Minus),
    ("Oemcomma", Key::Comma),
    ("OemPeriod", Key::Period),
    ("OemQuestion", Key::Slash),
    ("OemPipe", Key::Backslash),
    ("OemBackslash", Key::Backslash),
    ("OemSemicolon", Key::Semicolon),
    ("OemQuotes", Key::Quote),
    ("OemOpenBrackets", Key::BracketLeft),
    ("OemCloseBrackets", Key::BracketRight),
    ("Oemtilde", Key::Backquote),
    ("Add", Key::NumpadAdd),
    ("Subtract", Key::NumpadSubtract),
    ("Multiply", Key::NumpadMultiply),
    ("Divide", Key::NumpadDivide),
    ("Decimal", Key::NumpadDecimal),
    ("MediaNextTrack", Key::MediaNext),
    ("MediaPreviousTrack", Key::MediaPrev),
    ("LaunchApplication1", Key::LaunchApp1),
    ("LaunchApplication2", Key::LaunchApp2),
];

/// The hotkey text of a ShareX hotkey. `Ok(None)` for `None` or blank text (no hotkey). `Err` gives the
/// reason the text is not a hotkey.
pub(crate) fn chord(text: &str, win: bool) -> Result<Option<KeyChord>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("none") {
        return Ok(None);
    }
    let mut modifiers = if win { Modifiers::SUPER } else { Modifiers::empty() };
    let mut key = None;
    for token in trimmed.split([',', '+']).map(str::trim).filter(|token| !token.is_empty()) {
        if let Some(modifier) = modifier(token) {
            modifiers |= modifier;
        } else if let Some(found) = key_named(token) {
            if key.replace(found).is_some() {
                return Err("has more than one key".to_owned());
            }
        } else {
            return Err(format!("names an unknown key `{token}`"));
        }
    }
    match key {
        Some(key) => Ok(Some(KeyChord::new(modifiers, key))),
        None => Err("has no key".to_owned()),
    }
}

fn modifier(token: &str) -> Option<Modifiers> {
    match token.to_ascii_lowercase().as_str() {
        "control" | "ctrl" => Some(Modifiers::CTRL),
        "alt" => Some(Modifiers::ALT),
        "shift" => Some(Modifiers::SHIFT),
        "win" | "lwin" | "rwin" => Some(Modifiers::SUPER),
        _ => None,
    }
}

fn key_named(token: &str) -> Option<Key> {
    if let Some((_, key)) = WINFORMS_NAMES.iter().find(|(name, _)| name.eq_ignore_ascii_case(token)) {
        return Some(*key);
    }
    // WinForms calls the digit keys D0 to D9 and the keypad NumPad0 to NumPad9.
    if let Some(digit) = prefixed(token, "D").filter(|digit| digit.len() == 1) {
        return key_by_name(digit);
    }
    if let Some(digit) = prefixed(token, "NumPad").filter(|digit| digit.len() == 1) {
        return key_by_name(&format!("Numpad{digit}"));
    }
    key_by_name(token)
}

/// The rest of `token` after `prefix`, when it starts with it, ignoring case.
fn prefixed<'a>(token: &'a str, prefix: &str) -> Option<&'a str> {
    let head = token.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then(|| &token[prefix.len()..])
}

fn key_by_name(name: &str) -> Option<Key> {
    Key::ALL.iter().copied().find(|key| key.name().eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn parses_keys_and_modifiers_in_either_order() {
        assert_eq!(chord("PrintScreen, Control", false).unwrap(), Some(KeyChord::new(Modifiers::CTRL, Key::PrintScreen)));
        assert_eq!(chord("Control, Shift, S", false).unwrap(), Some(KeyChord::new(Modifiers::CTRL | Modifiers::SHIFT, Key::S)));
        assert_eq!(chord("D5, Control", false).unwrap(), Some(KeyChord::new(Modifiers::CTRL, Key::Digit5)));
        assert_eq!(chord("NumPad3, Alt", false).unwrap(), Some(KeyChord::new(Modifiers::ALT, Key::Numpad3)));
        assert_eq!(chord("Oemplus, Control", false).unwrap(), Some(KeyChord::new(Modifiers::CTRL, Key::Equal)));
        assert_eq!(chord("OemMinus", false).unwrap(), Some(KeyChord::new(Modifiers::empty(), Key::Minus)));
        assert_eq!(chord("Snapshot", true).unwrap(), Some(KeyChord::new(Modifiers::SUPER, Key::PrintScreen)));
    }

    #[test]
    fn parses_media_and_launch_keys() {
        let cases = [
            ("VolumeUp", Key::VolumeUp),
            ("VolumeDown", Key::VolumeDown),
            ("VolumeMute", Key::VolumeMute),
            ("MediaPlayPause", Key::MediaPlayPause),
            ("MediaNextTrack", Key::MediaNext),
            ("MediaPreviousTrack", Key::MediaPrev),
            ("MediaStop", Key::MediaStop),
            ("LaunchMail", Key::LaunchMail),
            ("LaunchApplication1", Key::LaunchApp1),
            ("LaunchApplication2", Key::LaunchApp2),
        ];
        for (text, key) in cases {
            assert_eq!(chord(text, false).unwrap(), Some(KeyChord::new(Modifiers::empty(), key)), "{text}");
        }
    }

    #[test]
    fn none_and_blank_mean_no_hotkey() {
        assert_eq!(chord("None", false), Ok(None));
        assert_eq!(chord("  ", false), Ok(None));
    }

    #[test]
    fn rejects_unknown_and_incomplete_text() {
        assert!(chord("Control", false).is_err());
        assert!(chord("S, X, Control", false).is_err());
        assert!(chord("Frobnicate, Control", false).is_err());
    }
}
