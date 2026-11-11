//! Records the keys a GPUI window never sees on Windows: GPUI has no name for Print Screen, Pause,
//! Scroll Lock or the media and launch keys and drops their events, so the settings window's
//! shortcut recorder can't catch them. While a recording lives, a low-level keyboard hook on its
//! own thread reports those keys with the modifiers held. Keys still reach the system as usual.
//! Which keys and when they count is a pure rule, so it is tested on Linux.

use laqta_domain::{Key, KeyChord, Modifiers};

use crate::keys;

/// The keys a GPUI window can't see on Windows.
#[cfg(any(windows, test))]
const BLIND: [Key; 13] = [
    Key::PrintScreen,
    Key::Pause,
    Key::ScrollLock,
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
];

/// The chord for a key event the hook saw, or `None` when the window sees that key itself.
/// Windows often sends Print Screen as a key release only, so it counts on release; every other
/// key counts on press.
#[cfg(any(windows, test))]
pub(crate) fn chord_for(vk: u32, modifiers: Modifiers, pressed: bool) -> Option<KeyChord> {
    // Ctrl+Pause arrives as Break (`VK_CANCEL`).
    let key = if vk == keys::VK_CANCEL && modifiers.contains(Modifiers::CTRL) {
        Key::Pause
    } else {
        BLIND.into_iter().find(|k| keys::vk(*k) == Some(vk))?
    };
    let counts = if key == Key::PrintScreen { !pressed } else { pressed };
    counts.then_some(KeyChord { modifiers, key })
}

#[cfg(windows)]
pub(crate) use imp::start;

#[cfg(windows)]
mod imp {
    use std::sync::{Mutex, PoisonError, mpsc};

    use laqta_domain::Modifiers;
    use laqta_ports::{ChordCallback, KeyRecording, PortError};
    use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT};
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, MSG, PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx,
        WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
    };

    /// Where the hook sends chords. The hook procedure is a plain function, so this is a static;
    /// one recording runs at a time (the settings window records one row at a time).
    static SINK: Mutex<Option<ChordCallback>> = Mutex::new(None);

    /// Starts the hook thread and returns once the hook is installed.
    pub(crate) fn start(on_chord: ChordCallback) -> Result<KeyRecording, PortError> {
        *SINK.lock().unwrap_or_else(PoisonError::into_inner) = Some(on_chord);
        let (ready_tx, ready_rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("laqta-key-recorder".into())
            .spawn(move || {
                // SAFETY: `hook_proc` has the HOOKPROC signature; a low-level hook needs no module
                // handle and runs on this thread, whose message loop below keeps it serviced.
                let hook = match unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), None, 0) } {
                    Ok(hook) => hook,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e.message()));
                        return;
                    }
                };
                // SAFETY: a plain query of this thread's id.
                let _ = ready_tx.send(Ok(unsafe { GetCurrentThreadId() }));
                let mut msg = MSG::default();
                // SAFETY: a standard message loop on this thread; it ends at WM_QUIT (0) or on error (-1).
                while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {}
                // SAFETY: the hook was installed above and is removed once.
                let _ = unsafe { UnhookWindowsHookEx(hook) };
            })
            .map_err(|e| PortError::Failed(e.to_string()))?;
        match ready_rx.recv() {
            Ok(Ok(thread)) => Ok(KeyRecording::new(Stop(thread))),
            Ok(Err(why)) => Err(PortError::Failed(format!("keyboard hook: {why}"))),
            Err(_) => Err(PortError::Failed("keyboard hook thread ended".into())),
        }
    }

    /// Ends the hook thread and forgets the sink when the recording is dropped.
    struct Stop(u32);

    impl Drop for Stop {
        fn drop(&mut self) {
            *SINK.lock().unwrap_or_else(PoisonError::into_inner) = None;
            // SAFETY: posting WM_QUIT to the hook thread ends its message loop.
            let _ = unsafe { PostThreadMessageW(self.0, WM_QUIT, WPARAM(0), LPARAM(0)) };
        }
    }

    fn held(vk: VIRTUAL_KEY) -> bool {
        // SAFETY: a plain key-state query; the top bit means the key is down.
        (unsafe { GetAsyncKeyState(i32::from(vk.0)) } as u16 & 0x8000) != 0
    }

    fn modifiers() -> Modifiers {
        let mut m = Modifiers::empty();
        m.set(Modifiers::CTRL, held(VK_CONTROL));
        m.set(Modifiers::ALT, held(VK_MENU));
        m.set(Modifiers::SHIFT, held(VK_SHIFT));
        m.set(Modifiers::SUPER, held(VK_LWIN) || held(VK_RWIN));
        m
    }

    unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32 {
            // SAFETY: for HC_ACTION a low-level keyboard hook's lparam points at a KBDLLHOOKSTRUCT.
            let event = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
            let message = wparam.0 as u32;
            let pressed = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
            let released = message == WM_KEYUP || message == WM_SYSKEYUP;
            if (pressed || released)
                && let Some(chord) = super::chord_for(event.vkCode, modifiers(), pressed)
                && let Some(sink) = SINK.lock().ok().and_then(|s| s.clone())
            {
                sink(chord);
            }
        }
        // SAFETY: passes the event on unchanged, as every hook must.
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn print_screen_counts_on_release_and_other_blind_keys_on_press() {
        let prtsc = keys::vk(Key::PrintScreen).unwrap();
        assert_eq!(chord_for(prtsc, Modifiers::CTRL, true), None);
        assert_eq!(chord_for(prtsc, Modifiers::CTRL, false), Some(KeyChord { modifiers: Modifiers::CTRL, key: Key::PrintScreen }));
        let play = keys::vk(Key::MediaPlayPause).unwrap();
        assert_eq!(chord_for(play, Modifiers::empty(), true), Some(KeyChord { modifiers: Modifiers::empty(), key: Key::MediaPlayPause }));
        assert_eq!(chord_for(play, Modifiers::empty(), false), None);
    }

    #[test]
    fn ctrl_break_is_recorded_as_ctrl_pause() {
        let chord = KeyChord { modifiers: Modifiers::CTRL, key: Key::Pause };
        assert_eq!(chord_for(keys::VK_CANCEL, Modifiers::CTRL, true), Some(chord));
        assert_eq!(keys::hotkey_vk(chord), Some(keys::VK_CANCEL));
        assert_eq!(keys::hotkey_vk(KeyChord { modifiers: Modifiers::empty(), key: Key::Pause }), keys::vk(Key::Pause));
    }

    #[test]
    fn keys_the_window_sees_are_left_to_it() {
        assert_eq!(chord_for(keys::vk(Key::A).unwrap(), Modifiers::CTRL, true), None);
        assert_eq!(chord_for(keys::vk(Key::F5).unwrap(), Modifiers::empty(), true), None);
    }
}
