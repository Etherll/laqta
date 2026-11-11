//! Windows virtual-key codes for the platform-neutral keys. `RegisterHotKey` takes these.

use laqta_domain::Key;

/// The virtual key Windows sends with Ctrl held down on Pause (Ctrl+Pause is Break).
pub(crate) const VK_CANCEL: u32 = 0x03;

/// The virtual key a hotkey for `chord` must register: Ctrl+Pause arrives as `VK_CANCEL`, so a
/// `VK_PAUSE` registration with Ctrl would never fire.
pub(crate) fn hotkey_vk(chord: laqta_domain::KeyChord) -> Option<u32> {
    if chord.key == Key::Pause && chord.modifiers.contains(laqta_domain::Modifiers::CTRL) {
        return Some(VK_CANCEL);
    }
    vk(chord.key)
}

/// The `VK_*` code for `key`, or `None` if Windows has no key for it.
pub(crate) fn vk(key: Key) -> Option<u32> {
    let code: u32 = match key {
        Key::A => 0x41,
        Key::B => 0x42,
        Key::C => 0x43,
        Key::D => 0x44,
        Key::E => 0x45,
        Key::F => 0x46,
        Key::G => 0x47,
        Key::H => 0x48,
        Key::I => 0x49,
        Key::J => 0x4A,
        Key::K => 0x4B,
        Key::L => 0x4C,
        Key::M => 0x4D,
        Key::N => 0x4E,
        Key::O => 0x4F,
        Key::P => 0x50,
        Key::Q => 0x51,
        Key::R => 0x52,
        Key::S => 0x53,
        Key::T => 0x54,
        Key::U => 0x55,
        Key::V => 0x56,
        Key::W => 0x57,
        Key::X => 0x58,
        Key::Y => 0x59,
        Key::Z => 0x5A,
        Key::Digit0 => 0x30,
        Key::Digit1 => 0x31,
        Key::Digit2 => 0x32,
        Key::Digit3 => 0x33,
        Key::Digit4 => 0x34,
        Key::Digit5 => 0x35,
        Key::Digit6 => 0x36,
        Key::Digit7 => 0x37,
        Key::Digit8 => 0x38,
        Key::Digit9 => 0x39,
        Key::F1 => 0x70,
        Key::F2 => 0x71,
        Key::F3 => 0x72,
        Key::F4 => 0x73,
        Key::F5 => 0x74,
        Key::F6 => 0x75,
        Key::F7 => 0x76,
        Key::F8 => 0x77,
        Key::F9 => 0x78,
        Key::F10 => 0x79,
        Key::F11 => 0x7A,
        Key::F12 => 0x7B,
        Key::F13 => 0x7C,
        Key::F14 => 0x7D,
        Key::F15 => 0x7E,
        Key::F16 => 0x7F,
        Key::F17 => 0x80,
        Key::F18 => 0x81,
        Key::F19 => 0x82,
        Key::F20 => 0x83,
        Key::F21 => 0x84,
        Key::F22 => 0x85,
        Key::F23 => 0x86,
        Key::F24 => 0x87,
        Key::PrintScreen => 0x2C,
        Key::Pause => 0x13,
        Key::ScrollLock => 0x91,
        Key::Insert => 0x2D,
        Key::Delete => 0x2E,
        Key::Home => 0x24,
        Key::End => 0x23,
        Key::PageUp => 0x21,
        Key::PageDown => 0x22,
        Key::Left => 0x25,
        Key::Up => 0x26,
        Key::Right => 0x27,
        Key::Down => 0x28,
        Key::Space => 0x20,
        Key::Enter => 0x0D,
        Key::Escape => 0x1B,
        Key::Tab => 0x09,
        Key::Backspace => 0x08,
        Key::Numpad0 => 0x60,
        Key::Numpad1 => 0x61,
        Key::Numpad2 => 0x62,
        Key::Numpad3 => 0x63,
        Key::Numpad4 => 0x64,
        Key::Numpad5 => 0x65,
        Key::Numpad6 => 0x66,
        Key::Numpad7 => 0x67,
        Key::Numpad8 => 0x68,
        Key::Numpad9 => 0x69,
        Key::NumpadMultiply => 0x6A,
        Key::NumpadAdd => 0x6B,
        Key::NumpadSubtract => 0x6D,
        Key::NumpadDecimal => 0x6E,
        Key::NumpadDivide => 0x6F,
        Key::Minus => 0xBD,
        Key::Equal => 0xBB,
        Key::Comma => 0xBC,
        Key::Period => 0xBE,
        Key::Slash => 0xBF,
        Key::Backslash => 0xDC,
        Key::Semicolon => 0xBA,
        Key::Quote => 0xDE,
        Key::BracketLeft => 0xDB,
        Key::BracketRight => 0xDD,
        Key::Backquote => 0xC0,
        Key::VolumeUp => 0xAF,
        Key::VolumeDown => 0xAE,
        Key::VolumeMute => 0xAD,
        Key::MediaPlayPause => 0xB3,
        Key::MediaNext => 0xB0,
        Key::MediaPrev => 0xB1,
        Key::MediaStop => 0xB2,
        Key::LaunchMail => 0xB4,
        Key::LaunchApp1 => 0xB6,
        // Calculator has no VK_* code of its own: keyboards send it as VK_LAUNCH_APP2.
        Key::LaunchApp2 | Key::Calculator => 0xB7,
        _ => return None,
    };
    Some(code)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    /// Keys with no `VK_*` code.
    const UNMAPPED: &[Key] = &[];

    #[test]
    fn unmapped_keys_are_exactly_the_documented_ones() {
        for &key in Key::ALL {
            assert_eq!(vk(key).is_none(), UNMAPPED.contains(&key), "{key:?}");
        }
    }

    #[test]
    fn virtual_keys_are_distinct() {
        // Calculator is the same key as LaunchApp2 on Windows.
        let mapped: Vec<u32> = Key::ALL.iter().filter(|&&key| key != Key::Calculator).filter_map(|&key| vk(key)).collect();
        let codes: HashSet<u32> = mapped.iter().copied().collect();
        assert_eq!(codes.len(), mapped.len());
    }

    #[test]
    fn spot_checks_match_win32_constants() {
        assert_eq!(vk(Key::A), Some(0x41));
        assert_eq!(vk(Key::Digit7), Some(0x37));
        assert_eq!(vk(Key::PrintScreen), Some(0x2C));
        assert_eq!(vk(Key::F24), Some(0x87));
        assert_eq!(vk(Key::Backquote), Some(0xC0));
    }
}
