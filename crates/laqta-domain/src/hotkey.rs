//! Platform-neutral key chords.

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(transparent)]
    pub struct Modifiers: u8 {
        const CTRL = 1;
        const ALT = 2;
        const SHIFT = 4;
        const SUPER = 8;
    }
}

/// Declares `Key`, `Key::ALL` and `Key::name` from one table, so they cannot drift apart.
macro_rules! keys {
    ($($variant:ident => $name:literal,)*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
        #[non_exhaustive]
        pub enum Key {
            $($variant,)*
        }

        impl Key {
            /// Every key, in declaration order.
            pub const ALL: &[Key] = &[$(Key::$variant),*];

            /// The canonical name: what `KeyChord` displays. Parsing accepts it in any case.
            pub fn name(self) -> &'static str {
                match self {
                    $(Key::$variant => $name,)*
                }
            }
        }
    };
}

keys! {
    A => "A", B => "B", C => "C", D => "D", E => "E", F => "F", G => "G", H => "H", I => "I",
    J => "J", K => "K", L => "L", M => "M", N => "N", O => "O", P => "P", Q => "Q", R => "R",
    S => "S", T => "T", U => "U", V => "V", W => "W", X => "X", Y => "Y", Z => "Z",
    Digit0 => "0", Digit1 => "1", Digit2 => "2", Digit3 => "3", Digit4 => "4",
    Digit5 => "5", Digit6 => "6", Digit7 => "7", Digit8 => "8", Digit9 => "9",
    F1 => "F1", F2 => "F2", F3 => "F3", F4 => "F4", F5 => "F5", F6 => "F6", F7 => "F7", F8 => "F8",
    F9 => "F9", F10 => "F10", F11 => "F11", F12 => "F12", F13 => "F13", F14 => "F14",
    F15 => "F15", F16 => "F16", F17 => "F17", F18 => "F18", F19 => "F19", F20 => "F20",
    F21 => "F21", F22 => "F22", F23 => "F23", F24 => "F24",
    PrintScreen => "PrintScreen", Pause => "Pause", ScrollLock => "ScrollLock",
    Insert => "Insert", Delete => "Delete", Home => "Home", End => "End",
    PageUp => "PageUp", PageDown => "PageDown",
    Left => "Left", Right => "Right", Up => "Up", Down => "Down",
    Space => "Space", Enter => "Enter", Escape => "Escape", Tab => "Tab", Backspace => "Backspace",
    Numpad0 => "Numpad0", Numpad1 => "Numpad1", Numpad2 => "Numpad2", Numpad3 => "Numpad3",
    Numpad4 => "Numpad4", Numpad5 => "Numpad5", Numpad6 => "Numpad6", Numpad7 => "Numpad7",
    Numpad8 => "Numpad8", Numpad9 => "Numpad9",
    NumpadAdd => "NumpadAdd", NumpadSubtract => "NumpadSubtract",
    NumpadMultiply => "NumpadMultiply", NumpadDivide => "NumpadDivide",
    NumpadDecimal => "NumpadDecimal",
    Minus => "Minus", Equal => "Equal", Comma => "Comma", Period => "Period", Slash => "Slash",
    Backslash => "Backslash", Semicolon => "Semicolon", Quote => "Quote",
    BracketLeft => "BracketLeft", BracketRight => "BracketRight", Backquote => "Backquote",
    VolumeUp => "VolumeUp", VolumeDown => "VolumeDown", VolumeMute => "VolumeMute",
    MediaPlayPause => "MediaPlayPause", MediaNext => "MediaNext", MediaPrev => "MediaPrev",
    MediaStop => "MediaStop", LaunchMail => "LaunchMail", LaunchApp1 => "LaunchApp1",
    LaunchApp2 => "LaunchApp2", Calculator => "Calculator",
}

/// Modifier names in the order `Display` prints them.
const MODIFIER_NAMES: [(Modifiers, &str); 4] =
    [(Modifiers::CTRL, "Ctrl"), (Modifiers::ALT, "Alt"), (Modifiers::SHIFT, "Shift"), (Modifiers::SUPER, "Super")];

const MODIFIER_ALIASES: &[(&str, Modifiers)] = &[
    ("ctrl", Modifiers::CTRL),
    ("control", Modifiers::CTRL),
    ("alt", Modifiers::ALT),
    ("option", Modifiers::ALT),
    ("shift", Modifiers::SHIFT),
    ("super", Modifiers::SUPER),
    ("win", Modifiers::SUPER),
    ("windows", Modifiers::SUPER),
    ("meta", Modifiers::SUPER),
    ("cmd", Modifiers::SUPER),
];

/// Key names that differ from `Key::name`. Parsing tries names first, then these.
const KEY_ALIASES: &[(&str, Key)] = &[
    ("prtsc", Key::PrintScreen),
    ("prtscn", Key::PrintScreen),
    ("print", Key::PrintScreen),
    ("esc", Key::Escape),
    ("return", Key::Enter),
    ("del", Key::Delete),
    ("ins", Key::Insert),
    ("pgup", Key::PageUp),
    ("pgdn", Key::PageDown),
    ("num0", Key::Numpad0),
    ("num1", Key::Numpad1),
    ("num2", Key::Numpad2),
    ("num3", Key::Numpad3),
    ("num4", Key::Numpad4),
    ("num5", Key::Numpad5),
    ("num6", Key::Numpad6),
    ("num7", Key::Numpad7),
    ("num8", Key::Numpad8),
    ("num9", Key::Numpad9),
    ("volup", Key::VolumeUp),
    ("voldown", Key::VolumeDown),
    ("mute", Key::VolumeMute),
    ("playpause", Key::MediaPlayPause),
    ("nexttrack", Key::MediaNext),
    ("prevtrack", Key::MediaPrev),
    ("mail", Key::LaunchMail),
    ("calc", Key::Calculator),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct KeyChord {
    pub modifiers: Modifiers,
    pub key: Key,
}

impl KeyChord {
    pub fn new(modifiers: Modifiers, key: Key) -> Self {
        Self { modifiers, key }
    }

    /// True when the chord has no modifiers, so it fires on the bare key.
    pub fn is_bare(&self) -> bool {
        self.modifiers.is_empty()
    }
}

impl fmt::Display for KeyChord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (modifier, name) in MODIFIER_NAMES {
            if self.modifiers.contains(modifier) {
                write!(f, "{name}+")?;
            }
        }
        f.write_str(self.key.name())
    }
}

/// Why a string is not a `KeyChord`.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HotkeyParseError {
    #[error("hotkey is empty")]
    Empty,
    #[error("unknown key `{0}`")]
    UnknownKey(String),
    #[error("hotkey has modifiers but no key")]
    NoKey,
    #[error("hotkey has more than one key")]
    DuplicateKey,
}

impl FromStr for KeyChord {
    type Err = HotkeyParseError;

    /// Parses `Ctrl+Shift+PrintScreen`-style text. Parts are split on `+`, trimmed, and matched
    /// case-insensitively. Exactly one part must be a key; the rest must be modifiers.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.trim().is_empty() {
            return Err(HotkeyParseError::Empty);
        }
        let mut modifiers = Modifiers::empty();
        let mut key: Option<Key> = None;
        for part in s.split('+').map(str::trim) {
            if let Some(modifier) = lookup(MODIFIER_ALIASES, part) {
                modifiers |= modifier;
                continue;
            }
            let found = lookup_key(part).ok_or_else(|| HotkeyParseError::UnknownKey(part.to_owned()))?;
            if key.is_some() {
                return Err(HotkeyParseError::DuplicateKey);
            }
            key = Some(found);
        }
        let key = key.ok_or(HotkeyParseError::NoKey)?;
        Ok(Self::new(modifiers, key))
    }
}

/// Chords bound more than once, with the indexes of their bindings, in order of first appearance.
pub fn find_conflicts<T>(bindings: &[(KeyChord, T)]) -> Vec<(KeyChord, Vec<usize>)> {
    let mut groups: Vec<(KeyChord, Vec<usize>)> = Vec::new();
    let mut group_of: HashMap<KeyChord, usize> = HashMap::new();
    for (index, (chord, _)) in bindings.iter().enumerate() {
        match group_of.get(chord) {
            Some(&group) => groups[group].1.push(index),
            None => {
                group_of.insert(*chord, groups.len());
                groups.push((*chord, vec![index]));
            }
        }
    }
    groups.retain(|(_, indexes)| indexes.len() > 1);
    groups
}

fn lookup<T: Copy>(table: &[(&str, T)], token: &str) -> Option<T> {
    table.iter().find(|(name, _)| name.eq_ignore_ascii_case(token)).map(|(_, value)| *value)
}

fn lookup_key(token: &str) -> Option<Key> {
    Key::ALL.iter().copied().find(|key| key.name().eq_ignore_ascii_case(token)).or_else(|| lookup(KEY_ALIASES, token))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn combos() -> [Modifiers; 5] {
        [Modifiers::empty(), Modifiers::CTRL, Modifiers::CTRL | Modifiers::SHIFT, Modifiers::ALT | Modifiers::SUPER, Modifiers::all()]
    }

    #[test]
    fn every_key_round_trips_through_display_and_parse() {
        for &key in Key::ALL {
            for modifiers in combos() {
                let chord = KeyChord::new(modifiers, key);
                let text = chord.to_string();
                assert_eq!(text.parse::<KeyChord>(), Ok(chord), "{text}");
            }
        }
    }

    #[test]
    fn displays_modifiers_in_fixed_order() {
        assert_eq!(KeyChord::new(Modifiers::all(), Key::A).to_string(), "Ctrl+Alt+Shift+Super+A");
        assert_eq!(KeyChord::new(Modifiers::SHIFT | Modifiers::CTRL, Key::PrintScreen).to_string(), "Ctrl+Shift+PrintScreen");
        assert_eq!(KeyChord::new(Modifiers::empty(), Key::PrintScreen).to_string(), "PrintScreen");
    }

    #[test]
    fn parses_any_case_with_spaces_around_parts() {
        assert_eq!(
            " ctrl + SHIFT +printscreen ".parse::<KeyChord>(),
            Ok(KeyChord::new(Modifiers::CTRL | Modifiers::SHIFT, Key::PrintScreen))
        );
    }

    #[test]
    fn accepts_modifier_aliases() {
        let cases = [
            ("ctrl", Modifiers::CTRL),
            ("Control", Modifiers::CTRL),
            ("alt", Modifiers::ALT),
            ("Option", Modifiers::ALT),
            ("shift", Modifiers::SHIFT),
            ("super", Modifiers::SUPER),
            ("win", Modifiers::SUPER),
            ("Windows", Modifiers::SUPER),
            ("meta", Modifiers::SUPER),
            ("cmd", Modifiers::SUPER),
        ];
        for (alias, modifier) in cases {
            let text = format!("{alias}+A");
            assert_eq!(text.parse::<KeyChord>(), Ok(KeyChord::new(modifier, Key::A)), "{text}");
        }
    }

    #[test]
    fn accepts_key_aliases() {
        let cases = [
            ("prtsc", Key::PrintScreen),
            ("PrtScn", Key::PrintScreen),
            ("print", Key::PrintScreen),
            ("esc", Key::Escape),
            ("Return", Key::Enter),
            ("del", Key::Delete),
            ("ins", Key::Insert),
            ("pgup", Key::PageUp),
            ("PgDn", Key::PageDown),
            ("num0", Key::Numpad0),
            ("num9", Key::Numpad9),
            ("numpad5", Key::Numpad5),
            ("0", Key::Digit0),
            ("9", Key::Digit9),
        ];
        for (text, key) in cases {
            assert_eq!(text.parse::<KeyChord>(), Ok(KeyChord::new(Modifiers::empty(), key)), "{text}");
        }
    }

    #[test]
    fn rejects_malformed_chords() {
        assert_eq!("".parse::<KeyChord>(), Err(HotkeyParseError::Empty));
        assert_eq!("   ".parse::<KeyChord>(), Err(HotkeyParseError::Empty));
        assert_eq!("Ctrl".parse::<KeyChord>(), Err(HotkeyParseError::NoKey));
        assert_eq!("Ctrl+Shift".parse::<KeyChord>(), Err(HotkeyParseError::NoKey));
        assert_eq!("A+B".parse::<KeyChord>(), Err(HotkeyParseError::DuplicateKey));
        assert_eq!("Ctrl+A+A".parse::<KeyChord>(), Err(HotkeyParseError::DuplicateKey));
        assert_eq!("Ctrl+Foo".parse::<KeyChord>(), Err(HotkeyParseError::UnknownKey("Foo".into())));
        assert_eq!("+A".parse::<KeyChord>(), Err(HotkeyParseError::UnknownKey(String::new())));
    }

    #[test]
    fn media_and_launch_keys_round_trip_by_name_and_alias() {
        let cases = [
            ("VolumeUp", Key::VolumeUp),
            ("volup", Key::VolumeUp),
            ("VolumeDown", Key::VolumeDown),
            ("VolumeMute", Key::VolumeMute),
            ("mute", Key::VolumeMute),
            ("MediaPlayPause", Key::MediaPlayPause),
            ("playpause", Key::MediaPlayPause),
            ("MediaNext", Key::MediaNext),
            ("nexttrack", Key::MediaNext),
            ("MediaPrev", Key::MediaPrev),
            ("prevtrack", Key::MediaPrev),
            ("MediaStop", Key::MediaStop),
            ("LaunchMail", Key::LaunchMail),
            ("mail", Key::LaunchMail),
            ("LaunchApp1", Key::LaunchApp1),
            ("LaunchApp2", Key::LaunchApp2),
            ("Calculator", Key::Calculator),
            ("calc", Key::Calculator),
        ];
        for (text, key) in cases {
            assert_eq!(text.parse::<KeyChord>(), Ok(KeyChord::new(Modifiers::empty(), key)), "{text}");
        }
        assert_eq!(Key::MediaPlayPause.name(), "MediaPlayPause");
        assert_eq!(KeyChord::new(Modifiers::CTRL, Key::MediaPlayPause).to_string(), "Ctrl+MediaPlayPause");
    }

    #[test]
    fn key_names_are_unique_ignoring_case() {
        let mut seen = std::collections::HashSet::new();
        for key in Key::ALL {
            assert!(seen.insert(key.name().to_ascii_lowercase()), "duplicate name {}", key.name());
        }
    }

    #[test]
    fn names_follow_variant_names_except_digits() {
        assert_eq!(Key::Digit0.name(), "0");
        assert_eq!(Key::F24.name(), "F24");
        assert_eq!(Key::PageUp.name(), "PageUp");
        assert_eq!(Key::Numpad5.name(), "Numpad5");
        assert_eq!(Key::NumpadAdd.name(), "NumpadAdd");
        assert_eq!(Key::Backquote.name(), "Backquote");
    }

    #[test]
    fn reports_repeated_chords_in_first_appearance_order() {
        let ctrl_a = KeyChord::new(Modifiers::CTRL, Key::A);
        let bare_a = KeyChord::new(Modifiers::empty(), Key::A);
        let bare_b = KeyChord::new(Modifiers::empty(), Key::B);

        let bindings = [(ctrl_a, "open"), (bare_b, "save"), (bare_a, "copy"), (ctrl_a, "capture"), (bare_b, "print")];
        assert_eq!(find_conflicts(&bindings), vec![(ctrl_a, vec![0, 3]), (bare_b, vec![1, 4])]);

        let bindings = [(bare_b, 0), (ctrl_a, 1), (bare_b, 2), (ctrl_a, 3)];
        assert_eq!(find_conflicts(&bindings), vec![(bare_b, vec![0, 2]), (ctrl_a, vec![1, 3])]);
    }

    #[test]
    fn no_conflicts_when_every_chord_is_unique() {
        let bindings = [(KeyChord::new(Modifiers::CTRL, Key::A), 0), (KeyChord::new(Modifiers::empty(), Key::A), 1)];
        assert!(find_conflicts(&bindings).is_empty());
        assert!(find_conflicts::<u8>(&[]).is_empty());
    }

    #[test]
    fn bare_means_no_modifiers() {
        assert!(KeyChord::new(Modifiers::empty(), Key::F5).is_bare());
        assert!(!KeyChord::new(Modifiers::ALT, Key::F5).is_bare());
    }
}
