//! Records the keys a GPUI window never sees on Windows: GPUI has no name for Print Screen, Pause,
//! Scroll Lock or the media and launch keys and drops their events, so the settings window's
//! shortcut recorder can't catch them. While a recording lives, a low-level keyboard hook on its
//! own thread reports those keys with the modifiers held. Keys still reach the system as usual.
//! Which keys and when they count is a pure rule, so it is tested on Linux.
//!
//! The same kind of hook can also catch the global shortcuts (`hotkeys_via_hook`), for games that
//! starve `RegisterHotKey`. That hook takes the keys of a matching shortcut, so the game never sees
//! them. Which events it takes is the pure [`Interceptor`].

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

/// Whether the shortcut's key counts when it is released rather than pressed. Windows often sends
/// Print Screen as a release only.
#[cfg(any(windows, test))]
fn counts_on_release(key: Key) -> bool {
    key == Key::PrintScreen
}

#[cfg(any(windows, test))]
const VK_SHIFT: u32 = 0x10;
#[cfg(any(windows, test))]
const VK_CONTROL: u32 = 0x11;
#[cfg(any(windows, test))]
const VK_MENU: u32 = 0x12;
#[cfg(any(windows, test))]
const VK_LSHIFT: u32 = 0xA0;
#[cfg(any(windows, test))]
const VK_RSHIFT: u32 = 0xA1;
#[cfg(any(windows, test))]
const VK_LCONTROL: u32 = 0xA2;
#[cfg(any(windows, test))]
const VK_RCONTROL: u32 = 0xA3;
#[cfg(any(windows, test))]
const VK_LMENU: u32 = 0xA4;
#[cfg(any(windows, test))]
const VK_RMENU: u32 = 0xA5;
#[cfg(any(windows, test))]
const VK_LWIN: u32 = 0x5B;
#[cfg(any(windows, test))]
const VK_RWIN: u32 = 0x5C;

/// The eight modifier keys, left and right, which a low-level hook reports apart. Their order is the
/// order of the bits in `Interceptor::held`.
#[cfg(any(windows, test))]
const MODIFIER_KEYS: [u32; 8] = [VK_LSHIFT, VK_RSHIFT, VK_LCONTROL, VK_RCONTROL, VK_LMENU, VK_RMENU, VK_LWIN, VK_RWIN];

/// The bit of `Interceptor::held` for the modifier key `vk`, or `None` for any other key. The codes
/// that do not say left or right count as the left key.
#[cfg(any(windows, test))]
fn modifier_bit(vk: u32) -> Option<u8> {
    let key = match vk {
        VK_SHIFT => VK_LSHIFT,
        VK_CONTROL => VK_LCONTROL,
        VK_MENU => VK_LMENU,
        other => other,
    };
    MODIFIER_KEYS.iter().position(|&k| k == key).map(|at| 1u8 << at)
}

/// What the shortcut hook does with a key event.
#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Not part of a shortcut: the event goes on to whoever has the keyboard.
    Pass,
    /// Part of a shortcut: the event is taken, but nothing fires on it.
    Swallow,
    /// The key of a shortcut that fires when it is released (Print Screen) went down: the event is
    /// taken and nothing fires yet.
    Hold(KeyChord),
    /// Shortcut `id` (`chord`) fires, and the event is taken.
    Fire { id: i32, chord: KeyChord },
}

/// Decides which key events belong to a registered shortcut. A shortcut fires once per press, and
/// the repeats and the release of the key that fired it are taken too, so the program with the
/// keyboard never sees half of it. It follows the modifier keys from their own events, not from the
/// key state of the moment, which has moved on when the hook gets to a fast tap.
#[cfg(any(windows, test))]
#[derive(Debug)]
pub(crate) struct Interceptor {
    chords: Vec<(i32, KeyChord)>,
    /// The keys whose press was taken and that have not been released yet, with their shortcut.
    down: Vec<(u32, i32, KeyChord)>,
    /// The keys whose press went on to the window (no shortcut matched), so their release does too.
    passed: Vec<u32>,
    /// The modifier keys that are down, one bit each (see `MODIFIER_KEYS`).
    held: u8,
}

#[cfg(any(windows, test))]
impl Interceptor {
    pub(crate) const fn new() -> Self {
        Interceptor { chords: Vec::new(), down: Vec::new(), passed: Vec::new(), held: 0 }
    }

    /// Replaces the shortcuts. Keys held from the old ones are forgotten; the modifiers are not.
    pub(crate) fn set(&mut self, chords: Vec<(i32, KeyChord)>) {
        self.chords = chords;
        self.down.clear();
        self.passed.clear();
    }

    /// Takes the keys that are down right now from `is_down`, which the hook does when it starts and
    /// whenever it is installed again. That sets the modifiers, and drops a taken key whose release
    /// never came (the hook was gone for a moment) so it cannot swallow that key later.
    pub(crate) fn sync(&mut self, is_down: impl Fn(u32) -> bool) {
        self.held = MODIFIER_KEYS.iter().filter(|&&vk| is_down(vk)).filter_map(|&vk| modifier_bit(vk)).fold(0, |bits, bit| bits | bit);
        self.down.retain(|&(vk, ..)| is_down(vk));
        self.passed.retain(|&vk| is_down(vk));
    }

    fn modifiers(&self) -> Modifiers {
        let mut modifiers = Modifiers::empty();
        modifiers.set(Modifiers::SHIFT, self.held & 0b0000_0011 != 0);
        modifiers.set(Modifiers::CTRL, self.held & 0b0000_1100 != 0);
        modifiers.set(Modifiers::ALT, self.held & 0b0011_0000 != 0);
        modifiers.set(Modifiers::SUPER, self.held & 0b1100_0000 != 0);
        modifiers
    }

    /// The verdict on a key event: `vk` went down (`pressed`) or up.
    pub(crate) fn event(&mut self, vk: u32, pressed: bool) -> Verdict {
        if let Some(bit) = modifier_bit(vk) {
            if pressed {
                self.held |= bit;
            } else {
                self.held &= !bit;
            }
            return Verdict::Pass;
        }
        let modifiers = self.modifiers();
        if let Some(at) = self.down.iter().position(|&(held, ..)| held == vk) {
            let (_, id, chord) = self.down[at];
            if !pressed {
                self.down.swap_remove(at);
                // The modifiers may have been let go first, so the release does not look at them.
                return if counts_on_release(chord.key) { Verdict::Fire { id, chord } } else { Verdict::Swallow };
            }
            // A repeat only belongs to the shortcut while its modifiers are still down. Otherwise the
            // release of that press was lost, and this is a new press that is not ours to take.
            if chord.modifiers == modifiers {
                return Verdict::Swallow;
            }
            self.down.swap_remove(at);
        }
        if !pressed && let Some(at) = self.passed.iter().position(|&k| k == vk) {
            self.passed.swap_remove(at);
            return Verdict::Pass;
        }
        let Some(&(id, chord)) = self.chords.iter().find(|(_, c)| c.modifiers == modifiers && keys::hotkey_vk(*c) == Some(vk)) else {
            // A press that is not ours: remember it, so its release (once the modifiers are up, it
            // may look like a shortcut) goes to the same place.
            if pressed && !self.passed.contains(&vk) && self.chords.iter().any(|(_, c)| keys::hotkey_vk(*c) == Some(vk)) {
                self.passed.push(vk);
            }
            return Verdict::Pass;
        };
        match (pressed, counts_on_release(chord.key)) {
            (true, false) => {
                self.down.push((vk, id, chord));
                Verdict::Fire { id, chord }
            }
            (true, true) => {
                self.down.push((vk, id, chord));
                Verdict::Hold(chord)
            }
            // Windows sometimes sends Print Screen as a release with no press.
            (false, true) => Verdict::Fire { id, chord },
            (false, false) => Verdict::Pass,
        }
    }
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

    pub(super) fn held(vk: VIRTUAL_KEY) -> bool {
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

/// The keyboard hook that catches the global shortcuts, for `hotkeys_via_hook`. It runs on a thread
/// of its own, so a slow shell thread can never hold up the keyboard of the whole desktop: Windows
/// drops a hook that does not answer in time. A match is posted to the shell window as `WM_HOTKEY`,
/// the message `RegisterHotKey` would have sent, and the key is taken.
///
/// Windows removes such a hook without a word when it times out, so the thread installs a new hook and
/// removes the old one every 5 seconds, and the keyboard is dead for 5 seconds at most. When no new
/// hook can be installed, the thread ends and tells the shell thread, which registers the shortcuts
/// with `RegisterHotKey` instead.
#[cfg(windows)]
pub(crate) mod hotkey_hook {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError, mpsc};
    use std::thread::JoinHandle;

    use laqta_domain::{KeyChord, Modifiers};
    use laqta_ports::PortError;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetMessageW, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, KillTimer, MSG, PM_NOREMOVE, PeekMessageW, PostMessageW,
        PostThreadMessageW, SetTimer, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_HOTKEY, WM_KEYDOWN, WM_KEYUP, WM_QUIT,
        WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMER, WM_USER,
    };

    use super::{Interceptor, Verdict};

    /// A virtual-key code no key sends. A shortcut with the Windows key held would open the Start menu
    /// when that key comes up, since the hook took the shortcut's own key; a key event in between,
    /// even this one, tells Windows the Windows key was used for something.
    const MASK_KEY: u16 = 0xE8;

    /// How often the hook is installed again, in milliseconds.
    const REINSTALL_EVERY: u32 = 5000;

    /// Called on the hook thread when it could not install the hook again and has ended.
    type OnLost = Arc<dyn Fn() + Send + Sync>;

    /// What the hook procedure works from: the shell window to post to, and the shortcuts.
    struct Watch {
        window: isize,
        interceptor: Interceptor,
        on_lost: Option<OnLost>,
    }

    /// The running hook thread.
    struct Runner {
        thread: u32,
        handle: JoinHandle<()>,
    }

    static WATCH: Mutex<Watch> = Mutex::new(Watch { window: 0, interceptor: Interceptor::new(), on_lost: None });
    static RUNNER: Mutex<Option<Runner>> = Mutex::new(None);
    static ENABLED: AtomicBool = AtomicBool::new(false);
    /// Set by the hook thread just before it reports that the hook was lost, and cleared by `stop`, so
    /// a report that arrives after the hook was stopped or started again is told apart and ignored.
    static LOST: AtomicBool = AtomicBool::new(false);

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Sets whether shortcuts go through the hook. Returns whether that changed it.
    pub(crate) fn set_enabled(on: bool) -> bool {
        ENABLED.swap(on, Ordering::SeqCst) != on
    }

    /// Whether shortcuts go through the hook.
    pub(crate) fn enabled() -> bool {
        ENABLED.load(Ordering::SeqCst)
    }

    /// Whether the hook thread reported that it lost the hook, which this answers once.
    pub(crate) fn take_lost() -> bool {
        LOST.swap(false, Ordering::SeqCst)
    }

    /// Starts watching for `chords` (each with the id its `WM_HOTKEY` carries), posting to the shell
    /// window `window`. The hook thread starts with the first call and runs until `stop`. `on_lost`
    /// is called, on the hook thread, if the hook is lost for good.
    pub(crate) fn watch(window: isize, chords: Vec<(i32, KeyChord)>, on_lost: OnLost) -> Result<(), PortError> {
        {
            let mut watch = lock(&WATCH);
            watch.window = window;
            watch.interceptor.set(chords);
            watch.on_lost = Some(on_lost);
        }
        let mut runner = lock(&RUNNER);
        // A thread that ended on its own (the hook was lost) is replaced by a new one.
        if runner.as_ref().is_some_and(|running| running.handle.is_finished())
            && let Some(ended) = runner.take()
        {
            let _ = ended.handle.join();
        }
        if runner.is_none() {
            LOST.store(false, Ordering::SeqCst);
            match spawn() {
                Ok(started) => *runner = Some(started),
                Err(error) => {
                    lock(&WATCH).interceptor.set(Vec::new());
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    /// Stops the hook and forgets the shortcuts. The hook is gone when this returns.
    pub(crate) fn stop() {
        {
            let mut watch = lock(&WATCH);
            watch.interceptor.set(Vec::new());
            watch.on_lost = None;
        }
        LOST.store(false, Ordering::SeqCst);
        if let Some(runner) = lock(&RUNNER).take() {
            // SAFETY: posting WM_QUIT to the hook thread ends its message loop.
            let _ = unsafe { PostThreadMessageW(runner.thread, WM_QUIT, WPARAM(0), LPARAM(0)) };
            let _ = runner.handle.join();
        }
    }

    fn install() -> windows::core::Result<HHOOK> {
        // SAFETY: `hook_proc` has the HOOKPROC signature; a low-level hook needs no module handle and
        // runs on the calling thread, whose message loop keeps it serviced.
        unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), None, 0) }
    }

    fn key_is_down(vk: u32) -> bool {
        super::imp::held(VIRTUAL_KEY(vk as u16))
    }

    /// Starts the hook thread and returns once the hook is installed.
    fn spawn() -> Result<Runner, PortError> {
        let (ready_tx, ready_rx) = mpsc::channel();
        let handle = std::thread::Builder::new()
            .name("laqta-hotkey-hook".into())
            .spawn(move || {
                let mut hook = match install() {
                    Ok(hook) => hook,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e.message()));
                        return;
                    }
                };
                lock(&WATCH).interceptor.sync(key_is_down);
                let mut msg = MSG::default();
                // A peek gives this thread its message queue, so `stop` can post WM_QUIT to it at once.
                // SAFETY: `msg` is a local; PM_NOREMOVE leaves the queue as it is.
                let _ = unsafe { PeekMessageW(&mut msg, None, WM_USER, WM_USER, PM_NOREMOVE) };
                // SAFETY: a timer of this thread without a window: its WM_TIMER messages come to the queue
                // below, with the returned id in wparam.
                let timer = unsafe { SetTimer(None, 0, REINSTALL_EVERY, None) };
                // SAFETY: a plain query of this thread's id.
                let _ = ready_tx.send(Ok(unsafe { GetCurrentThreadId() }));
                let mut lost = false;
                // SAFETY: a standard message loop on this thread; it ends at WM_QUIT (0) or on error (-1).
                while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
                    if timer != 0 && msg.message == WM_TIMER && msg.wParam.0 == timer {
                        match install() {
                            Ok(fresh) => {
                                // SAFETY: the old hook was installed by this thread and is removed once. The
                                // new one is already in place, so no key event goes past unseen.
                                let _ = unsafe { UnhookWindowsHookEx(hook) };
                                hook = fresh;
                                lock(&WATCH).interceptor.sync(key_is_down);
                            }
                            Err(e) => {
                                tracing::warn!("the keyboard hook for hotkeys could not be installed again: {}", e.message());
                                lost = true;
                                break;
                            }
                        }
                    }
                }
                if timer != 0 {
                    // SAFETY: the timer was set above, on this thread.
                    let _ = unsafe { KillTimer(None, timer) };
                }
                // SAFETY: the hook was installed by this thread and is removed once.
                let _ = unsafe { UnhookWindowsHookEx(hook) };
                if lost {
                    // The shell thread is told last, so it finds this thread finished.
                    LOST.store(true, Ordering::SeqCst);
                    let on_lost = lock(&WATCH).on_lost.clone();
                    if let Some(on_lost) = on_lost {
                        on_lost();
                    }
                }
            })
            .map_err(|e| PortError::Failed(e.to_string()))?;
        match ready_rx.recv() {
            Ok(Ok(thread)) => Ok(Runner { thread, handle }),
            Ok(Err(why)) => Err(PortError::Failed(format!("keyboard hook: {why}"))),
            Err(_) => Err(PortError::Failed("keyboard hook thread ended".into())),
        }
    }

    unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32 {
            // SAFETY: for HC_ACTION a low-level keyboard hook's lparam points at a KBDLLHOOKSTRUCT.
            let event = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
            let message = wparam.0 as u32;
            let pressed = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
            let released = message == WM_KEYUP || message == WM_SYSKEYUP;
            if (pressed || released) && takes(event.vkCode, pressed) {
                // A non-zero result keeps the event from the hooks after this one and from the program.
                return LRESULT(1);
            }
        }
        // SAFETY: passes the event on unchanged, as every hook must.
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    /// Whether the event belongs to a shortcut, which is then posted to the shell window when it fires.
    /// A shortcut with the Windows key is masked from its first press on, so it does not matter which
    /// key comes up first.
    fn takes(vk: u32, pressed: bool) -> bool {
        let (verdict, window) = {
            let mut watch = lock(&WATCH);
            (watch.interceptor.event(vk, pressed), watch.window)
        };
        match verdict {
            Verdict::Pass => false,
            Verdict::Swallow => true,
            Verdict::Hold(chord) => {
                if chord.modifiers.contains(Modifiers::SUPER) {
                    mask_start_menu();
                }
                true
            }
            Verdict::Fire { id, chord } => {
                let hwnd = HWND(window as *mut core::ffi::c_void);
                // SAFETY: posting to the shell thread's window; no pointers are passed. A window that is
                // gone makes the call fail, which is ignored. The id is the one `RegisterHotKey` would send.
                let _ = unsafe { PostMessageW(Some(hwnd), WM_HOTKEY, WPARAM(id as usize), LPARAM(0)) };
                if chord.modifiers.contains(Modifiers::SUPER) {
                    mask_start_menu();
                }
                true
            }
        }
    }

    /// Sends the unassigned key, so the Windows key coming up after a shortcut does not open the Start menu.
    fn mask_start_menu() {
        let key = |flags: KEYBD_EVENT_FLAGS| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(MASK_KEY), wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
        };
        let inputs = [key(KEYBD_EVENT_FLAGS(0)), key(KEYEVENTF_KEYUP)];
        // SAFETY: `inputs` is a live slice of `INPUT` records, and the size passed is `INPUT`'s own.
        let _ = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
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

    fn chord(modifiers: Modifiers, key: Key) -> KeyChord {
        KeyChord { modifiers, key }
    }

    fn interceptor(chords: &[(i32, KeyChord)]) -> Interceptor {
        let mut interceptor = Interceptor::new();
        interceptor.set(chords.to_vec());
        interceptor
    }

    const CTRL_SHIFT_S: KeyChord = KeyChord { modifiers: Modifiers::CTRL.union(Modifiers::SHIFT), key: Key::S };

    #[test]
    fn a_release_whose_press_went_to_windows_does_not_fire() {
        let prtsc = keys::vk(Key::PrintScreen).unwrap();
        let bare = chord(Modifiers::empty(), Key::PrintScreen);
        let mut hook = interceptor(&[(1, bare)]);
        // Alt+PrtSc is Windows' own: the press passes, Alt goes up first, and the release passes too.
        assert_eq!(hook.event(VK_LMENU, true), Verdict::Pass);
        assert_eq!(hook.event(prtsc, true), Verdict::Pass);
        assert_eq!(hook.event(VK_LMENU, false), Verdict::Pass);
        assert_eq!(hook.event(prtsc, false), Verdict::Pass);
        // A bare press and release still fires.
        assert_eq!(hook.event(prtsc, true), Verdict::Hold(bare));
        assert_eq!(hook.event(prtsc, false), Verdict::Fire { id: 1, chord: bare });
    }

    #[test]
    fn a_shortcut_fires_on_its_press_and_its_repeats_and_release_are_taken() {
        let s = keys::vk(Key::S).unwrap();
        let mut hook = interceptor(&[(7, CTRL_SHIFT_S)]);
        assert_eq!(hook.event(VK_LCONTROL, true), Verdict::Pass);
        assert_eq!(hook.event(VK_LSHIFT, true), Verdict::Pass);
        assert_eq!(hook.event(s, true), Verdict::Fire { id: 7, chord: CTRL_SHIFT_S });
        assert_eq!(hook.event(s, true), Verdict::Swallow);
        assert_eq!(hook.event(s, true), Verdict::Swallow);
        // Ctrl was let go first; the release is still taken.
        assert_eq!(hook.event(VK_LCONTROL, false), Verdict::Pass);
        assert_eq!(hook.event(s, false), Verdict::Swallow);
        // A new press fires again.
        assert_eq!(hook.event(VK_LCONTROL, true), Verdict::Pass);
        assert_eq!(hook.event(s, true), Verdict::Fire { id: 7, chord: CTRL_SHIFT_S });
    }

    #[test]
    fn print_screen_holds_its_press_and_fires_on_the_release() {
        let print = keys::vk(Key::PrintScreen).unwrap();
        let ctrl_print = chord(Modifiers::CTRL, Key::PrintScreen);
        let mut hook = interceptor(&[(1, ctrl_print)]);
        hook.event(VK_RCONTROL, true);
        assert_eq!(hook.event(print, true), Verdict::Hold(ctrl_print));
        assert_eq!(hook.event(print, true), Verdict::Swallow);
        // The release counts even when Ctrl is already up.
        hook.event(VK_RCONTROL, false);
        assert_eq!(hook.event(print, false), Verdict::Fire { id: 1, chord: ctrl_print });
        assert_eq!(hook.event(print, false), Verdict::Pass);
    }

    #[test]
    fn print_screen_sent_as_a_release_only_still_fires() {
        let print = keys::vk(Key::PrintScreen).unwrap();
        let ctrl_print = chord(Modifiers::CTRL, Key::PrintScreen);
        let mut hook = interceptor(&[(1, ctrl_print)]);
        hook.event(VK_LCONTROL, true);
        assert_eq!(hook.event(print, false), Verdict::Fire { id: 1, chord: ctrl_print });
    }

    #[test]
    fn a_shortcut_with_the_windows_key_is_held_from_its_first_press() {
        let print = keys::vk(Key::PrintScreen).unwrap();
        let win_print = chord(Modifiers::SUPER, Key::PrintScreen);
        let mut hook = interceptor(&[(2, win_print)]);
        hook.event(VK_LWIN, true);
        // The hook masks the Start menu when it gets this verdict, so letting go of Win first is safe.
        assert_eq!(hook.event(print, true), Verdict::Hold(win_print));
        hook.event(VK_LWIN, false);
        assert_eq!(hook.event(print, false), Verdict::Fire { id: 2, chord: win_print });
    }

    #[test]
    fn other_keys_and_other_modifiers_pass() {
        let s = keys::vk(Key::S).unwrap();
        let mut hook = interceptor(&[(7, CTRL_SHIFT_S)]);
        hook.event(VK_LCONTROL, true);
        assert_eq!(hook.event(s, true), Verdict::Pass);
        hook.event(VK_LCONTROL, false);
        assert_eq!(hook.event(s, true), Verdict::Pass);
        hook.event(VK_LCONTROL, true);
        hook.event(VK_LSHIFT, true);
        hook.event(VK_LMENU, true);
        assert_eq!(hook.event(s, true), Verdict::Pass);
        hook.event(VK_LMENU, false);
        assert_eq!(hook.event(keys::vk(Key::A).unwrap(), true), Verdict::Pass);
        // A release whose press was not taken is not ours.
        assert_eq!(hook.event(s, false), Verdict::Pass);
    }

    #[test]
    fn a_lost_release_does_not_swallow_the_key_later() {
        let s = keys::vk(Key::S).unwrap();
        let ctrl_s = chord(Modifiers::CTRL, Key::S);
        let mut hook = interceptor(&[(7, ctrl_s)]);
        hook.event(VK_LCONTROL, true);
        assert_eq!(hook.event(s, true), Verdict::Fire { id: 7, chord: ctrl_s });
        // The release of S never arrives; Ctrl comes up, and S is typed on its own.
        hook.event(VK_LCONTROL, false);
        assert_eq!(hook.event(s, true), Verdict::Pass);
        assert_eq!(hook.event(s, false), Verdict::Pass);
        // The shortcut works again.
        hook.event(VK_LCONTROL, true);
        assert_eq!(hook.event(s, true), Verdict::Fire { id: 7, chord: ctrl_s });
    }

    #[test]
    fn a_repeat_with_other_modifiers_is_not_taken() {
        let s = keys::vk(Key::S).unwrap();
        let mut hook = interceptor(&[(7, CTRL_SHIFT_S)]);
        hook.event(VK_LCONTROL, true);
        hook.event(VK_LSHIFT, true);
        assert_eq!(hook.event(s, true), Verdict::Fire { id: 7, chord: CTRL_SHIFT_S });
        hook.event(VK_LCONTROL, false);
        assert_eq!(hook.event(s, true), Verdict::Pass);
    }

    #[test]
    fn syncing_drops_keys_that_are_no_longer_down_and_reads_the_modifiers() {
        let s = keys::vk(Key::S).unwrap();
        let ctrl_s = chord(Modifiers::CTRL, Key::S);
        let mut hook = interceptor(&[(7, ctrl_s)]);
        hook.event(VK_LCONTROL, true);
        assert_eq!(hook.event(s, true), Verdict::Fire { id: 7, chord: ctrl_s });
        // The release of S was lost while the hook was away; Ctrl is still down.
        hook.sync(|vk| vk == VK_LCONTROL);
        assert_eq!(hook.event(s, true), Verdict::Fire { id: 7, chord: ctrl_s });
        // A key that is still down keeps being taken.
        hook.sync(|vk| vk == VK_LCONTROL || vk == s);
        assert_eq!(hook.event(s, true), Verdict::Swallow);
        // Modifiers that came up unseen are forgotten, and ones pressed unseen are known.
        hook.sync(|vk| vk == VK_RSHIFT);
        assert_eq!(hook.event(s, false), Verdict::Pass);
        assert_eq!(hook.event(s, true), Verdict::Pass);
    }

    #[test]
    fn modifiers_follow_their_own_events_left_or_right() {
        let s = keys::vk(Key::S).unwrap();
        let ctrl_s = chord(Modifiers::CTRL, Key::S);
        let mut hook = interceptor(&[(7, ctrl_s)]);
        // Both Ctrl keys down: Ctrl stays held until both are up.
        hook.event(VK_LCONTROL, true);
        hook.event(VK_RCONTROL, true);
        hook.event(VK_LCONTROL, false);
        assert_eq!(hook.event(s, true), Verdict::Fire { id: 7, chord: ctrl_s });
        hook.event(s, false);
        hook.event(VK_RCONTROL, false);
        assert_eq!(hook.event(s, true), Verdict::Pass);
        // The codes that do not say which side count as the left key.
        hook.event(s, false);
        hook.event(VK_CONTROL, true);
        assert_eq!(hook.event(s, true), Verdict::Fire { id: 7, chord: ctrl_s });
    }

    #[test]
    fn ctrl_pause_is_matched_as_break() {
        let ctrl_pause = chord(Modifiers::CTRL, Key::Pause);
        let mut hook = interceptor(&[(3, ctrl_pause)]);
        hook.event(VK_LCONTROL, true);
        assert_eq!(hook.event(keys::VK_CANCEL, true), Verdict::Fire { id: 3, chord: ctrl_pause });
        assert_eq!(hook.event(keys::vk(Key::Pause).unwrap(), true), Verdict::Pass);
    }

    #[test]
    fn new_shortcuts_forget_the_keys_held_from_the_old_ones() {
        let s = keys::vk(Key::S).unwrap();
        let mut hook = interceptor(&[(7, CTRL_SHIFT_S)]);
        hook.event(VK_LCONTROL, true);
        hook.event(VK_LSHIFT, true);
        assert_eq!(hook.event(s, true), Verdict::Fire { id: 7, chord: CTRL_SHIFT_S });
        hook.set(vec![(8, CTRL_SHIFT_S)]);
        // The modifiers are still known, and the key held from before is not.
        assert_eq!(hook.event(s, true), Verdict::Fire { id: 8, chord: CTRL_SHIFT_S });
        hook.set(Vec::new());
        assert_eq!(hook.event(s, true), Verdict::Pass);
    }
}
