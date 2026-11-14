//! Hotkey hints for buttons that run an action.

use laqta_domain::Action;
use laqta_domain::hotkey::KeyChord;
use laqta_domain::record::Os;

/// The hotkey bound to `action`, as short text such as "Ctrl+PrtSc"; the first binding wins.
/// `bindings` is `Settings::hotkey_bindings()`, which is empty while hotkeys are disabled.
pub fn hotkey_label(bindings: &[(KeyChord, Action)], action: Action, os: Os) -> Option<String> {
    let (chord, _) = bindings.iter().find(|(_, a)| *a == action)?;
    Some(keycaps(*chord, os).join(if os == Os::MacOs { "" } else { "+" }))
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
    /// Backspace on its own: remove the shortcut. Delete is a key like any other (it can be bound).
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
        "backspace" if plain => return Recorded::Clear,
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
        "xf86audioraisevolume" => "VolumeUp",
        "xf86audiolowervolume" => "VolumeDown",
        "xf86audiomute" => "VolumeMute",
        "xf86audioplay" => "MediaPlayPause",
        "xf86audionext" => "MediaNext",
        "xf86audioprev" => "MediaPrev",
        "xf86audiostop" => "MediaStop",
        "xf86mail" => "LaunchMail",
        "xf86launch0" => "LaunchApp1",
        "xf86launch1" => "LaunchApp2",
        "xf86calculator" => "Calculator",
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

/// The keycaps a chord shows as, with PrintScreen shortened: ["Ctrl", "Shift", "PrtSc"]. On
/// macOS the modifiers are the symbols printed on a Mac keyboard: ["⌃", "⇧", "F13"].
pub fn keycaps(chord: KeyChord, os: Os) -> Vec<String> {
    chord
        .to_string()
        .split('+')
        .map(|p| match (p, os) {
            ("PrintScreen", Os::MacOs) => "F13".to_owned(),
            ("PrintScreen", _) => "PrtSc".to_owned(),
            (_, Os::MacOs) => mac_modifier(p).unwrap_or(p).to_owned(),
            _ => p.to_owned(),
        })
        .collect()
}

fn mac_modifier(name: &str) -> Option<&'static str> {
    Some(match name {
        "Ctrl" => "⌃",
        "Alt" => "⌥",
        "Shift" => "⇧",
        "Super" => "⌘",
        _ => return None,
    })
}

/// An in-window shortcut such as "Ctrl+Z" as `os` writes it. On macOS the Command key stands in
/// for Ctrl, so it reads "⌘Z".
pub fn shortcut_text(keys: &str, os: Os) -> String {
    if os != Os::MacOs {
        return keys.to_owned();
    }
    keys.split('+').map(|p| if p == "Ctrl" { "⌘" } else { mac_modifier(p).unwrap_or(p) }).collect()
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
    /// Registered, but the desktop uses another key for it (KDE keeps the key saved in its own
    /// settings): the desktop's key, as it writes it.
    DesktopKey(String),
    /// Registered, but the desktop gave it no key, so it can't fire.
    NoKey,
}

/// Whether the desktop's key text (KDE: "Ctrl+'", "Print", "Meta+Shift+S") names `chord`.
/// Spelling differs between desktops, so names are compared loosely, in any order. A portal lists
/// every key an action has, separated by ", " ("Print, Ctrl+Print"): any one of them may match.
pub fn same_key(chord: KeyChord, desktop: &str) -> bool {
    /// One spelling per key: Laqta's names (`Minus`, `Numpad1`, `VolumeUp`) and the desktop's
    /// (Qt/KDE: `-`, `Num+1`, `Volume Up`) meet here.
    fn norm(part: &str) -> String {
        let p = part.trim().to_ascii_lowercase();
        let same = match p.as_str() {
            "control" | "ctrl" => "ctrl",
            "meta" | "super" | "win" | "logo" => "super",
            "printscreen" | "prtsc" | "print" | "sysreq" => "print",
            "quote" | "apostrophe" | "'" => "'",
            "return" | "enter" => "enter",
            "esc" | "escape" => "escape",
            "pgup" | "pageup" | "page up" => "pageup",
            "pgdown" | "pagedown" | "page down" => "pagedown",
            "minus" | "-" => "-",
            "equal" | "=" => "=",
            "comma" | "," => ",",
            "period" | "." => ".",
            "slash" | "/" => "/",
            "backslash" | "\\" => "\\",
            "semicolon" | ";" => ";",
            "bracketleft" | "[" => "[",
            "bracketright" | "]" => "]",
            "backquote" | "grave" | "`" => "`",
            "delete" | "del" => "delete",
            "insert" | "ins" => "insert",
            "scrolllock" | "scroll lock" => "scrolllock",
            "volumeup" | "volume up" => "volumeup",
            "volumedown" | "volume down" => "volumedown",
            "volumemute" | "volume mute" | "mute" => "volumemute",
            "mediaplaypause" | "media play" | "toggle media play/pause" | "play" => "mediaplaypause",
            "medianext" | "media next" => "medianext",
            "mediaprev" | "media previous" => "mediaprev",
            "mediastop" | "media stop" => "mediastop",
            "launchmail" | "launch mail" | "mail" => "launchmail",
            "calculator" | "calc" => "calculator",
            "numpadadd" => "numpad+",
            "numpadsubtract" => "numpad-",
            "numpadmultiply" => "numpad*",
            "numpaddivide" => "numpad/",
            "numpaddecimal" => "numpad.",
            _ => return p,
        };
        same.into()
    }
    fn parts(text: &str) -> Vec<String> {
        let mut pieces: Vec<String> = text.split('+').filter(|p| !p.trim().is_empty()).map(norm).collect();
        // "Ctrl++" is Ctrl and Plus: an empty piece after a '+' is the '+' key itself.
        if text.ends_with("++") {
            pieces.push("+".into());
        }
        // Qt writes a keypad key as "Num+<key>": one key, Laqta's `Numpad<key>`.
        let mut out = Vec::with_capacity(pieces.len());
        let mut pieces = pieces.into_iter().peekable();
        while let Some(piece) = pieces.next() {
            match pieces.peek() {
                Some(next) if piece == "num" => {
                    out.push(format!("numpad{next}"));
                    pieces.next();
                }
                _ => out.push(piece),
            }
        }
        out.sort();
        out
    }
    let wanted = parts(&chord.to_string());
    desktop.split(", ").any(|key| parts(key) == wanted)
}

/// The status of each row, from the draft (`(name, chord, enabled)` per row), the last
/// registration report's accepted and failed chords, and the keys the desktop says it uses
/// (`system_keys`, KDE only; "" means none).
pub fn row_status(
    rows: &[(String, Option<KeyChord>, bool)],
    registered: &[KeyChord],
    failed: &[(KeyChord, String)],
    system_keys: &[(KeyChord, String)],
) -> Vec<RowStatus> {
    rows.iter()
        .enumerate()
        .map(|(i, (_, chord, enabled))| {
            let Some(chord) = chord else { return RowStatus::Unset };
            if !enabled {
                return RowStatus::Disabled;
            }
            // The first enabled row with a key wins (`Settings::workflow_for_hotkey`), so say which one never runs.
            let same = |j: &usize, (_, c, e): &&(String, Option<KeyChord>, bool)| *j != i && *e && *c == Some(*chord);
            if let Some((_, (first, _, _))) = rows.iter().enumerate().find(|(j, r)| *j < i && same(j, r)) {
                return RowStatus::Conflict(format!("Never runs: “{first}” has the same key"));
            }
            if let Some((_, (later, _, _))) = rows.iter().enumerate().find(|(j, r)| same(j, r)) {
                return RowStatus::Conflict(format!("Also used by “{later}”, which never runs"));
            }
            if let Some((_, why)) = failed.iter().find(|(c, _)| c == chord) {
                return RowStatus::Failed(why.clone());
            }
            if !registered.contains(chord) {
                return RowStatus::Pending;
            }
            match system_keys.iter().find(|(c, _)| c == chord) {
                Some((_, key)) if key.trim().is_empty() => RowStatus::NoKey,
                Some((_, key)) if !same_key(*chord, key) => RowStatus::DesktopKey(key.clone()),
                _ => RowStatus::Active,
            }
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
    fn media_keys_record_from_gpui_names() {
        assert_eq!(record("xf86audioplay", Mods::default()), Recorded::Chord(chord("MediaPlayPause")));
        let cases = [
            ("xf86audioraisevolume", "VolumeUp"),
            ("xf86audiolowervolume", "VolumeDown"),
            ("xf86audiomute", "VolumeMute"),
            ("xf86audionext", "MediaNext"),
            ("xf86audioprev", "MediaPrev"),
            ("xf86audiostop", "MediaStop"),
            ("xf86mail", "LaunchMail"),
            ("xf86launch0", "LaunchApp1"),
            ("xf86launch1", "LaunchApp2"),
            ("xf86calculator", "Calculator"),
        ];
        for (gpui, name) in cases {
            assert_eq!(record(gpui, Mods::default()), Recorded::Chord(chord(name)), "{gpui}");
        }
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
        assert_eq!(record("delete", Mods::default()), Recorded::Chord(chord("Delete")));
        assert_eq!(record("delete", Mods { ctrl: true, ..Mods::default() }), Recorded::Chord(chord("Ctrl+Delete")));
        assert_eq!(record("é", Mods::default()), Recorded::Unusable);
    }

    #[test]
    fn keycaps_split_and_shorten() {
        assert_eq!(keycaps(chord("Ctrl+Shift+PrintScreen"), Os::Linux), ["Ctrl", "Shift", "PrtSc"]);
        assert_eq!(keycaps(chord("Ctrl+Shift+PrintScreen"), Os::Windows), ["Ctrl", "Shift", "PrtSc"]);
        assert_eq!(keycaps(chord("Ctrl+Alt+Super+1"), Os::MacOs), ["⌃", "⌥", "⌘", "1"]);
        assert_eq!(keycaps(chord("Shift+PrintScreen"), Os::MacOs), ["⇧", "F13"]);
    }

    #[test]
    fn in_window_shortcuts_use_command_on_macos() {
        assert_eq!(shortcut_text("Ctrl+Z", Os::Linux), "Ctrl+Z");
        assert_eq!(shortcut_text("Ctrl+Z", Os::MacOs), "⌘Z");
        assert_eq!(shortcut_text("Ctrl+Shift+S", Os::MacOs), "⌘⇧S");
        assert_eq!(shortcut_text("R", Os::MacOs), "R");
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
        let status = row_status(&rows, &[chord("PrintScreen")], &[(chord("F8"), "in use".into())], &[]);
        assert_eq!(status[0], RowStatus::Conflict("Also used by “Window”, which never runs".into()));
        assert_eq!(status[1], RowStatus::Conflict("Never runs: “Region” has the same key".into()));
        assert_eq!(status[2], RowStatus::Active);
        assert_eq!(status[3], RowStatus::Disabled);
        assert_eq!(status[4], RowStatus::Unset);
        assert_eq!(status[5], RowStatus::Failed("in use".into()));
    }

    #[test]
    fn the_desktops_own_key_wins_over_the_one_asked_for() {
        let rows = vec![
            ("Region".to_owned(), Some(chord("Ctrl+W")), true),
            ("Screen".to_owned(), Some(chord("PrintScreen")), true),
            ("Text".to_owned(), Some(chord("Ctrl+Quote")), true),
            ("Window".to_owned(), Some(chord("Alt+PrintScreen")), true),
        ];
        let registered = rows.iter().filter_map(|r| r.1).collect::<Vec<_>>();
        let system = vec![
            (chord("Ctrl+W"), "Alt+B".to_owned()),
            (chord("PrintScreen"), "Print".to_owned()),
            (chord("Ctrl+Quote"), "Ctrl+'".to_owned()),
            (chord("Alt+PrintScreen"), String::new()),
        ];
        let status = row_status(&rows, &registered, &[], &system);
        assert_eq!(status[0], RowStatus::DesktopKey("Alt+B".into()));
        assert_eq!(status[1], RowStatus::Active, "KDE writes PrintScreen as Print");
        assert_eq!(status[2], RowStatus::Active);
        assert_eq!(status[3], RowStatus::NoKey);
    }

    #[test]
    fn desktop_key_names_compare_loosely() {
        assert!(same_key(chord("Ctrl+Shift+PrintScreen"), "Shift+Ctrl+Print"));
        assert!(same_key(chord("Super+S"), "Meta+S"));
        assert!(!same_key(chord("Ctrl+W"), "Ctrl+Q"));
        // Qt/KDE spell symbols, the keypad and media keys their own way.
        assert!(same_key(chord("Ctrl+Minus"), "Ctrl+-"));
        assert!(same_key(chord("Ctrl+Shift+Comma"), "Shift+Ctrl+,"));
        assert!(same_key(chord("Ctrl+Delete"), "Ctrl+Del"));
        assert!(same_key(chord("VolumeUp"), "Volume Up"));
        assert!(same_key(chord("MediaPlayPause"), "Media Play"));
        assert!(same_key(chord("Ctrl+Numpad1"), "Ctrl+Num+1"));
        assert!(same_key(chord("Ctrl+NumpadAdd"), "Ctrl+Num++"));
        assert!(!same_key(chord("Ctrl+1"), "Ctrl+Num+1"), "the keypad 1 is not the top-row 1");
    }

    #[test]
    fn a_portal_description_with_several_keys_matches_any_of_them() {
        assert!(same_key(chord("Ctrl+PrintScreen"), "Print, Ctrl+Print"));
        assert!(same_key(chord("PrintScreen"), "Print, Ctrl+Print"));
        assert!(!same_key(chord("Alt+PrintScreen"), "Print, Ctrl+Print"));
        assert!(same_key(chord("Ctrl+Comma"), "Ctrl+,, Print"), "a comma key is not a separator");
        assert!(same_key(chord("Ctrl+NumpadAdd"), "Print, Ctrl+Num++"));
    }
    use laqta_domain::settings::Settings;

    #[test]
    fn finds_the_first_binding_for_an_action() {
        let bindings = Settings::default_for("/p".into()).hotkey_bindings();
        let (chord, action) = bindings[0];
        let label = hotkey_label(&bindings, action, Os::Linux).unwrap();
        assert_eq!(label, chord.to_string().replace("PrintScreen", "PrtSc"));
        assert!(!label.contains("PrintScreen"));
    }

    #[test]
    fn unbound_action_has_no_label() {
        assert_eq!(hotkey_label(&[], Action::PrintScreen, Os::Linux), None);
    }
}
