//! Laqta keys and modifiers as Carbon virtual key codes and modifier masks. Pure tables.
//!
//! The codes are the ANSI-layout `kVK_*` values from Carbon's `Events.h`. They name physical key
//! positions, so a chord on a non-US layout still fires from the same key.

use laqta_domain::{Key, Modifiers};

/// Carbon `cmdKey`.
const CMD_KEY: u32 = 0x0100;
/// Carbon `shiftKey`.
const SHIFT_KEY: u32 = 0x0200;
/// Carbon `optionKey`.
const OPTION_KEY: u32 = 0x0800;
/// Carbon `controlKey`.
const CONTROL_KEY: u32 = 0x1000;

/// The `kVK_*` code for `key`, or `None` if a Mac keyboard has no such key.
pub(crate) fn key_code(key: Key) -> Option<u32> {
    let code: u32 = match key {
        Key::A => 0x00,
        Key::S => 0x01,
        Key::D => 0x02,
        Key::F => 0x03,
        Key::H => 0x04,
        Key::G => 0x05,
        Key::Z => 0x06,
        Key::X => 0x07,
        Key::C => 0x08,
        Key::V => 0x09,
        Key::B => 0x0B,
        Key::Q => 0x0C,
        Key::W => 0x0D,
        Key::E => 0x0E,
        Key::R => 0x0F,
        Key::Y => 0x10,
        Key::T => 0x11,
        Key::Digit1 => 0x12,
        Key::Digit2 => 0x13,
        Key::Digit3 => 0x14,
        Key::Digit4 => 0x15,
        Key::Digit6 => 0x16,
        Key::Digit5 => 0x17,
        Key::Equal => 0x18,
        Key::Digit9 => 0x19,
        Key::Digit7 => 0x1A,
        Key::Minus => 0x1B,
        Key::Digit8 => 0x1C,
        Key::Digit0 => 0x1D,
        Key::BracketRight => 0x1E,
        Key::O => 0x1F,
        Key::U => 0x20,
        Key::BracketLeft => 0x21,
        Key::I => 0x22,
        Key::P => 0x23,
        Key::Enter => 0x24,
        Key::L => 0x25,
        Key::J => 0x26,
        Key::Quote => 0x27,
        Key::K => 0x28,
        Key::Semicolon => 0x29,
        Key::Backslash => 0x2A,
        Key::Comma => 0x2B,
        Key::Slash => 0x2C,
        Key::N => 0x2D,
        Key::M => 0x2E,
        Key::Period => 0x2F,
        Key::Tab => 0x30,
        Key::Space => 0x31,
        Key::Backquote => 0x32,
        // `kVK_Delete` is the key above Return, which the other platforms call Backspace.
        Key::Backspace => 0x33,
        Key::Escape => 0x35,
        Key::F17 => 0x40,
        Key::NumpadDecimal => 0x41,
        Key::NumpadMultiply => 0x43,
        Key::NumpadAdd => 0x45,
        Key::NumpadDivide => 0x4B,
        Key::NumpadSubtract => 0x4E,
        Key::F18 => 0x4F,
        Key::F19 => 0x50,
        Key::Numpad0 => 0x52,
        Key::Numpad1 => 0x53,
        Key::Numpad2 => 0x54,
        Key::Numpad3 => 0x55,
        Key::Numpad4 => 0x56,
        Key::Numpad5 => 0x57,
        Key::Numpad6 => 0x58,
        Key::Numpad7 => 0x59,
        Key::F20 => 0x5A,
        Key::Numpad8 => 0x5B,
        Key::Numpad9 => 0x5C,
        Key::F5 => 0x60,
        Key::F6 => 0x61,
        Key::F7 => 0x62,
        Key::F3 => 0x63,
        Key::F8 => 0x64,
        Key::F9 => 0x65,
        Key::F11 => 0x67,
        Key::F13 => 0x69,
        // A PC keyboard's Print Screen key sends F13 on a Mac, so a chord on it must use F13's
        // code. This is the one deliberate code shared by two keys.
        Key::PrintScreen => 0x69,
        Key::F16 => 0x6A,
        Key::F14 => 0x6B,
        Key::F10 => 0x6D,
        Key::F12 => 0x6F,
        Key::F15 => 0x71,
        Key::Home => 0x73,
        Key::PageUp => 0x74,
        // `kVK_ForwardDelete`: what Windows and Linux call Delete.
        Key::Delete => 0x75,
        Key::F4 => 0x76,
        Key::End => 0x77,
        Key::F2 => 0x78,
        Key::PageDown => 0x79,
        Key::F1 => 0x7A,
        Key::Left => 0x7B,
        Key::Right => 0x7C,
        Key::Down => 0x7D,
        Key::Up => 0x7E,
        // No code: Scroll Lock, Pause and Insert (the Help key sits where Insert is, but it is not
        // Insert), F21 to F24 (Mac keyboard drivers stop at F20), and the volume, media, mail, launch
        // and calculator keys, which macOS handles as system-defined events rather than `kVK_*` codes.
        // `Key` is non-exhaustive, so a new variant also lands here until it is mapped; the
        // `UNMAPPED` test then fails to say so.
        _ => return None,
    };
    Some(code)
}

/// The Carbon modifier mask for `m`. `SUPER` is Command and `ALT` is Option.
pub(crate) fn modifier_mask(m: Modifiers) -> u32 {
    let mut mask = 0;
    for (flag, carbon) in
        [(Modifiers::SUPER, CMD_KEY), (Modifiers::SHIFT, SHIFT_KEY), (Modifiers::ALT, OPTION_KEY), (Modifiers::CTRL, CONTROL_KEY)]
    {
        if m.contains(flag) {
            mask |= carbon;
        }
    }
    mask
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    /// Keys a Mac keyboard cannot send. Adding to this list needs a reason in `key_code`.
    const UNMAPPED: &[Key] = &[
        Key::ScrollLock,
        Key::Pause,
        Key::Insert,
        Key::F21,
        Key::F22,
        Key::F23,
        Key::F24,
        Key::VolumeUp,
        Key::VolumeDown,
        Key::VolumeMute,
        Key::MediaPlayPause,
        Key::MediaNext,
        Key::MediaPrev,
        Key::MediaStop,
        Key::LaunchMail,
        Key::LaunchApp1,
        Key::LaunchApp2,
        Key::Calculator,
    ];

    #[test]
    fn unmapped_keys_are_exactly_the_documented_ones() {
        for &key in Key::ALL {
            assert_eq!(key_code(key).is_none(), UNMAPPED.contains(&key), "{key:?}");
        }
        for &key in UNMAPPED {
            assert!(Key::ALL.contains(&key), "{key:?} is not a Key");
        }
    }

    #[test]
    fn only_print_screen_shares_a_code() {
        let mut by_code: HashMap<u32, Vec<Key>> = HashMap::new();
        for &key in Key::ALL {
            if let Some(code) = key_code(key) {
                by_code.entry(code).or_default().push(key);
            }
        }
        for (code, keys) in by_code {
            if code == 0x69 {
                assert_eq!(keys, [Key::F13, Key::PrintScreen], "code {code:#x}");
            } else {
                assert_eq!(keys.len(), 1, "code {code:#x} is shared by {keys:?}");
            }
        }
    }

    #[test]
    fn codes_fit_the_virtual_key_range() {
        for &key in Key::ALL {
            if let Some(code) = key_code(key) {
                assert!(code <= 0x7F, "{key:?} = {code:#x}");
            }
        }
    }

    #[test]
    fn spot_checks_match_events_h() {
        let cases = [
            (Key::A, 0x00),
            (Key::S, 0x01),
            (Key::Digit0, 0x1D),
            (Key::Digit5, 0x17),
            (Key::Enter, 0x24),
            (Key::Space, 0x31),
            (Key::Backspace, 0x33),
            (Key::Escape, 0x35),
            (Key::Delete, 0x75),
            (Key::F1, 0x7A),
            (Key::F12, 0x6F),
            (Key::F13, 0x69),
            (Key::PrintScreen, 0x69),
            (Key::Left, 0x7B),
            (Key::Up, 0x7E),
            (Key::Home, 0x73),
            (Key::PageDown, 0x79),
            (Key::Numpad0, 0x52),
            (Key::NumpadAdd, 0x45),
            (Key::Backquote, 0x32),
        ];
        for (key, code) in cases {
            assert_eq!(key_code(key), Some(code), "{key:?}");
        }
    }

    #[test]
    fn modifier_masks_are_the_carbon_ones() {
        assert_eq!(modifier_mask(Modifiers::empty()), 0);
        assert_eq!(modifier_mask(Modifiers::SUPER), 0x0100);
        assert_eq!(modifier_mask(Modifiers::SHIFT), 0x0200);
        assert_eq!(modifier_mask(Modifiers::ALT), 0x0800);
        assert_eq!(modifier_mask(Modifiers::CTRL), 0x1000);
        assert_eq!(modifier_mask(Modifiers::all()), 0x1B00);
        assert_eq!(modifier_mask(Modifiers::CTRL | Modifiers::SHIFT), 0x1200);
    }
}
