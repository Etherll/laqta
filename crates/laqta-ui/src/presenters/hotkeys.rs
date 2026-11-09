//! Hotkey hints for buttons that run an action.

use laqta_domain::Action;
use laqta_domain::hotkey::KeyChord;

/// The hotkey bound to `action`, as short text such as "Ctrl+PrtSc"; the first binding wins.
/// `bindings` is `Settings::hotkey_bindings()`, which is empty while hotkeys are disabled.
pub fn hotkey_label(bindings: &[(KeyChord, Action)], action: Action) -> Option<String> {
    bindings.iter().find(|(_, a)| *a == action).map(|(chord, _)| chord.to_string().replace("PrintScreen", "PrtSc"))
}

/// What one key press does while a shortcut is being recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recorded {
    /// A complete shortcut.
    Chord(KeyChord),
    /// Only modifiers so far: keep listening.
    Waiting,
    /// Esc on its own: stop recording, keep the old shortcut.
    Cancel,
    /// Backspace or Delete on its own: remove the shortcut.
    Clear,
    /// A key a global shortcut can't use.
    Unusable,
}

/// Modifier state for `record`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub super_: bool,
}

/// Turns a key press (GPUI's lowercase key name) into a recording step.
pub fn record(key: &str, mods: Mods) -> Recorded {
    let plain = !(mods.ctrl || mods.alt || mods.shift || mods.super_);
    match key {
        "control" | "shift" | "alt" | "super" | "platform" | "function" | "" => return Recorded::Waiting,
        "escape" if plain => return Recorded::Cancel,
        "backspace" | "delete" if plain => return Recorded::Clear,
        _ => {}
    }
    let name = match key {
        "-" => "Minus",
        "=" => "Equal",
        "," => "Comma",
        "." => "Period",
        "/" => "Slash",
        "\\" => "Backslash",
        ";" => "Semicolon",
        "'" => "Quote",
        "[" => "BracketLeft",
        "]" => "BracketRight",
        "`" => "Backquote",
        "sys_req" | "printscreen" => "PrintScreen",
        other => other,
    };
    let mut text = String::new();
    for (on, label) in [(mods.ctrl, "Ctrl+"), (mods.alt, "Alt+"), (mods.shift, "Shift+"), (mods.super_, "Super+")] {
        if on {
            text.push_str(label);
        }
    }
    text.push_str(name);
    match text.parse::<KeyChord>() {
        Ok(chord) => Recorded::Chord(chord),
        Err(_) => Recorded::Unusable,
    }
}

/// The keycaps a chord shows as, with PrintScreen shortened: ["Ctrl", "Shift", "PrtSc"].
pub fn keycaps(chord: KeyChord) -> Vec<String> {
    chord.to_string().split('+').map(|p| if p == "PrintScreen" { "PrtSc".to_owned() } else { p.to_owned() }).collect()
}

/// How one shortcut row stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowStatus {
    /// No key combination set.
    Unset,
    /// Switched off by the user.
    Disabled,
    /// Registered and listening.
    Active,
    /// Not registered yet (registration is running, or the change isn't saved).
    Pending,
    /// Another enabled shortcut uses the same keys.
    Conflict(String),
    /// The system turned it down.
    Failed(String),
}

/// The status of each row, from the draft (`(name, chord, enabled)` per row) and the last
/// registration report's accepted and failed chords.
pub fn row_status(rows: &[(String, Option<KeyChord>, bool)], registered: &[KeyChord], failed: &[(KeyChord, String)]) -> Vec<RowStatus> {
    rows.iter()
        .enumerate()
        .map(|(i, (_, chord, enabled))| {
            let Some(chord) = chord else { return RowStatus::Unset };
            if !enabled {
                return RowStatus::Disabled;
            }
            if let Some((other, _, _)) = rows.iter().enumerate().find(|(j, (_, c, e))| *j != i && *e && *c == Some(*chord)).map(|(_, r)| r)
            {
                return RowStatus::Conflict(format!("Also used by “{other}”"));
            }
            if let Some((_, why)) = failed.iter().find(|(c, _)| c == chord) {
                return RowStatus::Failed(why.clone());
            }
            if registered.contains(chord) { RowStatus::Active } else { RowStatus::Pending }
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn chord(s: &str) -> KeyChord {
        s.parse().unwrap()
    }

    #[test]
    fn recording_builds_a_chord_from_a_key_press() {
        let ctrl_shift = Mods { ctrl: true, shift: true, ..Mods::default() };
        assert_eq!(record("print", ctrl_shift), Recorded::Chord(chord("Ctrl+Shift+PrintScreen")));
        assert_eq!(record("a", Mods { super_: true, ..Mods::default() }), Recorded::Chord(chord("Super+A")));
        assert_eq!(record("f5", Mods::default()), Recorded::Chord(chord("F5")));
        assert_eq!(record("-", Mods { alt: true, ..Mods::default() }), Recorded::Chord(chord("Alt+Minus")));
        assert_eq!(record("shift", ctrl_shift), Recorded::Waiting);
        assert_eq!(record("escape", Mods::default()), Recorded::Cancel);
        assert_eq!(record("backspace", Mods::default()), Recorded::Clear);
        assert_eq!(record("é", Mods::default()), Recorded::Unusable);
    }

    #[test]
    fn keycaps_split_and_shorten() {
        assert_eq!(keycaps(chord("Ctrl+Shift+PrintScreen")), ["Ctrl", "Shift", "PrtSc"]);
    }

    #[test]
    fn rows_report_conflicts_failures_and_registration() {
        let rows = vec![
            ("Region".to_owned(), Some(chord("Ctrl+PrintScreen")), true),
            ("Window".to_owned(), Some(chord("Ctrl+PrintScreen")), true),
            ("Screen".to_owned(), Some(chord("PrintScreen")), true),
            ("Off".to_owned(), Some(chord("F9")), false),
            ("None".to_owned(), None, true),
            ("Taken".to_owned(), Some(chord("F8")), true),
        ];
        let status = row_status(&rows, &[chord("PrintScreen")], &[(chord("F8"), "in use".into())]);
        assert_eq!(status[0], RowStatus::Conflict("Also used by “Window”".into()));
        assert_eq!(status[2], RowStatus::Active);
        assert_eq!(status[3], RowStatus::Disabled);
        assert_eq!(status[4], RowStatus::Unset);
        assert_eq!(status[5], RowStatus::Failed("in use".into()));
    }
    use laqta_domain::settings::Settings;

    #[test]
    fn finds_the_first_binding_for_an_action() {
        let bindings = Settings::default_for("/p".into()).hotkey_bindings();
        let (chord, action) = bindings[0];
        let label = hotkey_label(&bindings, action).unwrap();
        assert_eq!(label, chord.to_string().replace("PrintScreen", "PrtSc"));
        assert!(!label.contains("PrintScreen"));
    }

    #[test]
    fn unbound_action_has_no_label() {
        assert_eq!(hotkey_label(&[], Action::PrintScreen), None);
    }
}
