//! The shell thread: one dedicated thread that owns a hidden window and runs its message loop.
//! It receives `WM_HOTKEY` (from `RegisterHotKey`) and the notification-area callbacks
//! (`Shell_NotifyIconW`), and shows the tray menu with `TrackPopupMenu`.
//!
//! Other threads talk to it through a command queue: they push a [`Command`], post
//! `WM_LAQTA_COMMAND`, and wait for the reply on a channel.
//!
//! The window is a hidden top-level tool window rather than a message-only (`HWND_MESSAGE`)
//! window: message-only windows don't receive broadcasts, so they would miss `TaskbarCreated`
//! (Explorer restarted, icon must be re-added), and `TrackPopupMenu` needs an owner that can be
//! made foreground for the menu to close when the user clicks elsewhere.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::mem::size_of;
use std::sync::{Mutex, OnceLock, mpsc};
use std::time::Duration;

use laqta_domain::{Action, KeyChord, Modifiers};
use laqta_ports::{ActionCallback, HotkeyCallback, HotkeyRegistrar, PortError, Tray, TrayMenu};
use windows::Win32::Foundation::{ERROR_HOTKEY_ALREADY_REGISTERED, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey, UnregisterHotKey,
};
use windows::Win32::UI::Shell::{NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW,
    IDI_APPLICATION, LoadIconW, MF_STRING, MSG, PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SetForegroundWindow,
    SetMenuDefaultItem, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage, WM_APP, WM_CONTEXTMENU, WM_DESTROY,
    WM_HOTKEY, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WNDCLASSW, WS_EX_TOOLWINDOW, WS_OVERLAPPED,
};
use windows::core::{HSTRING, w};

use crate::keys;

const WM_LAQTA_COMMAND: u32 = WM_APP + 1;
const WM_LAQTA_TRAY: u32 = WM_APP + 2;
const TRAY_ID: u32 = 1;
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);

type HotkeyResults = Vec<(KeyChord, Result<(), PortError>)>;

enum Command {
    Hotkeys { bindings: Vec<(KeyChord, Action)>, on_fire: HotkeyCallback, reply: mpsc::Sender<HotkeyResults> },
    Tray { menu: TrayMenu, on_select: ActionCallback, reply: mpsc::Sender<Result<(), PortError>> },
    UnregisterAll { reply: mpsc::Sender<()> },
    Shutdown { reply: mpsc::Sender<()> },
}

static QUEUE: Mutex<VecDeque<Command>> = Mutex::new(VecDeque::new());
/// The shell window's handle (as an integer, since `HWND` isn't `Send`), or why it failed.
static WINDOW: OnceLock<Result<isize, String>> = OnceLock::new();

/// State owned by the shell thread, touched only from its window procedure.
#[derive(Default)]
struct State {
    hotkeys: HashMap<i32, KeyChord>,
    on_hotkey: Option<HotkeyCallback>,
    tray: Option<(TrayMenu, ActionCallback)>,
    icon_added: bool,
    taskbar_created: u32,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

#[derive(Debug)]
pub(crate) struct WindowsHotkeys;

impl HotkeyRegistrar for WindowsHotkeys {
    fn register(&self, bindings: &[(KeyChord, Action)], on_fire: HotkeyCallback) -> HotkeyResults {
        let bindings = bindings.to_vec();
        let chords: Vec<KeyChord> = bindings.iter().map(|(c, _)| *c).collect();
        match send(|reply| Command::Hotkeys { bindings, on_fire, reply }) {
            Ok(results) => results,
            Err(e) => chords.into_iter().map(|c| (c, Err(e.clone()))).collect(),
        }
    }

    fn unregister_all(&self) {
        // Nothing can be registered before the shell thread exists, so don't start it just to clear.
        if let Some(Ok(_)) = WINDOW.get()
            && let Err(e) = send(|reply| Command::UnregisterAll { reply })
        {
            tracing::warn!("could not clear hotkeys: {e}");
        }
    }
}

#[derive(Debug)]
pub(crate) struct WindowsTray;

impl Tray for WindowsTray {
    fn show(&self, menu: TrayMenu, on_select: ActionCallback) -> Result<(), PortError> {
        send(|reply| Command::Tray { menu, on_select, reply })?
    }
}

/// Removes the tray icon, unregisters hotkeys and ends the shell thread, if it was started.
pub(crate) fn shutdown() {
    if let Some(Ok(_)) = WINDOW.get() {
        let _ = send(|reply| Command::Shutdown { reply });
    }
}

fn window() -> Result<HWND, PortError> {
    let raw = WINDOW.get_or_init(start).clone().map_err(PortError::Failed)?;
    Ok(HWND(raw as *mut c_void))
}

fn start() -> Result<isize, String> {
    let (ready, rx) = mpsc::channel();
    std::thread::Builder::new().name("laqta-shell".into()).spawn(move || run(ready)).map_err(|e| e.to_string())?;
    rx.recv().map_err(|_| "shell thread exited during start-up".to_string())?
}

fn send<T>(make: impl FnOnce(mpsc::Sender<T>) -> Command) -> Result<T, PortError> {
    let hwnd = window()?;
    let (reply, rx) = mpsc::channel();
    QUEUE.lock().unwrap_or_else(|p| p.into_inner()).push_back(make(reply));
    // SAFETY: posting to a window owned by the shell thread; no pointers are passed.
    unsafe { PostMessageW(Some(hwnd), WM_LAQTA_COMMAND, WPARAM(0), LPARAM(0)) }
        .map_err(|e| PortError::Failed(format!("shell thread: {e}")))?;
    rx.recv_timeout(REPLY_TIMEOUT).map_err(|_| PortError::Failed("shell thread did not answer".into()))
}

fn run(ready: mpsc::Sender<Result<isize, String>>) {
    // SAFETY: window creation and the message loop, all on this thread.
    unsafe {
        let instance = GetModuleHandleW(None).ok().map(|m| HINSTANCE(m.0));
        let class = w!("LaqtaShellWindow");
        let wc =
            WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: instance.unwrap_or_default(), lpszClassName: class, ..Default::default() };
        RegisterClassW(&wc);
        let hwnd = match CreateWindowExW(WS_EX_TOOLWINDOW, class, w!("Laqta"), WS_OVERLAPPED, 0, 0, 0, 0, None, None, instance, None) {
            Ok(h) => h,
            Err(e) => {
                let _ = ready.send(Err(format!("CreateWindowExW: {e}")));
                return;
            }
        };
        let taskbar_created = RegisterWindowMessageW(w!("TaskbarCreated"));
        STATE.with(|s| s.borrow_mut().taskbar_created = taskbar_created);
        let _ = ready.send(Ok(hwnd.0 as isize));
        let mut msg = MSG::default();
        // GetMessageW returns 0 on WM_QUIT and -1 on error.
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_LAQTA_COMMAND => {
            loop {
                let next = QUEUE.lock().unwrap_or_else(|p| p.into_inner()).pop_front();
                let Some(command) = next else { break };
                handle(hwnd, command);
            }
            LRESULT(0)
        }
        WM_HOTKEY => {
            let id = wparam.0 as i32;
            let fire = STATE.with(|s| {
                let s = s.borrow();
                s.hotkeys.get(&id).copied().zip(s.on_hotkey.clone())
            });
            if let Some((chord, callback)) = fire {
                callback(chord);
            }
            LRESULT(0)
        }
        WM_LAQTA_TRAY => {
            match (lparam.0 & 0xffff) as u32 {
                WM_LBUTTONUP => {
                    let fire = STATE.with(|s| s.borrow().tray.as_ref().map(|(m, cb)| (m.default_action, cb.clone())));
                    if let Some((action, callback)) = fire {
                        callback(action);
                    }
                }
                WM_RBUTTONUP | WM_CONTEXTMENU => show_menu(hwnd),
                _ => {}
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            // SAFETY: ends this thread's message loop.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        m if m != 0 && m == STATE.with(|s| s.borrow().taskbar_created) => {
            // Explorer restarted: add the icon again.
            let tooltip = STATE.with(|s| {
                let mut s = s.borrow_mut();
                s.icon_added = false;
                s.tray.as_ref().map(|(m, _)| m.tooltip.clone())
            });
            if let Some(tooltip) = tooltip
                && notify(hwnd, &tooltip, true).is_ok()
            {
                STATE.with(|s| s.borrow_mut().icon_added = true);
            }
            LRESULT(0)
        }
        // SAFETY: default handling for everything else.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn handle(hwnd: HWND, command: Command) {
    match command {
        Command::Hotkeys { bindings, on_fire, reply } => {
            let _ = reply.send(register_hotkeys(hwnd, &bindings, on_fire));
        }
        Command::Tray { menu, on_select, reply } => {
            let add = !STATE.with(|s| s.borrow().icon_added);
            let result = notify(hwnd, &menu.tooltip, add);
            STATE.with(|s| {
                let mut s = s.borrow_mut();
                if result.is_ok() {
                    s.icon_added = true;
                }
                s.tray = Some((menu, on_select));
            });
            let _ = reply.send(result);
        }
        Command::UnregisterAll { reply } => {
            unregister_hotkeys(hwnd);
            let _ = reply.send(());
        }
        Command::Shutdown { reply } => {
            unregister_hotkeys(hwnd);
            let icon = STATE.with(|s| std::mem::take(&mut s.borrow_mut().icon_added));
            // SAFETY: plain Win32 calls on this thread's window.
            unsafe {
                if icon {
                    let data =
                        NOTIFYICONDATAW { cbSize: size_of::<NOTIFYICONDATAW>() as u32, hWnd: hwnd, uID: TRAY_ID, ..Default::default() };
                    let _ = Shell_NotifyIconW(NIM_DELETE, &data);
                }
                let _ = DestroyWindow(hwnd);
            }
            let _ = reply.send(());
        }
    }
}

/// Unregisters every hotkey this window holds and forgets the callback.
fn unregister_hotkeys(hwnd: HWND) {
    let ids: Vec<i32> = STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.on_hotkey = None;
        s.hotkeys.drain().map(|(id, _)| id).collect()
    });
    for id in ids {
        // SAFETY: unregistering ids this window registered.
        let _ = unsafe { UnregisterHotKey(Some(hwnd), id) };
    }
}

fn register_hotkeys(hwnd: HWND, bindings: &[(KeyChord, Action)], on_fire: HotkeyCallback) -> HotkeyResults {
    unregister_hotkeys(hwnd);
    let mut registered = HashMap::new();
    let mut results = Vec::with_capacity(bindings.len());
    for (index, (chord, _)) in bindings.iter().enumerate() {
        let id = index as i32 + 1;
        let result = match keys::vk(chord.key) {
            None => Err(PortError::Failed(format!("{chord} uses a key Windows cannot register"))),
            // SAFETY: plain Win32 call on this thread's window.
            Some(vk) => unsafe { RegisterHotKey(Some(hwnd), id, modifiers(chord.modifiers) | MOD_NOREPEAT, vk) }
                .map_err(|e| PortError::Failed(refusal_text(&chord.to_string(), win32_code(&e), &e.message()))),
        };
        if result.is_ok() {
            registered.insert(id, *chord);
        }
        results.push((*chord, result));
    }
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.hotkeys = registered;
        s.on_hotkey = Some(on_fire);
    });
    results
}

/// The Win32 error code inside an `HRESULT`. Win32 errors arrive as `0x8007xxxx`; anything else
/// is returned as it is.
fn win32_code(e: &windows::core::Error) -> u32 {
    let hr = e.code().0 as u32;
    if hr >> 16 == 0x8007 { hr & 0xFFFF } else { hr }
}

/// The text shown for a hotkey that Windows refused.
fn refusal_text(chord: &str, code: u32, system_message: &str) -> String {
    if code == ERROR_HOTKEY_ALREADY_REGISTERED.0 {
        format!("{chord} is taken by another program")
    } else {
        format!("Windows refused {chord}: {system_message}")
    }
}

fn modifiers(m: Modifiers) -> HOT_KEY_MODIFIERS {
    let mut out = HOT_KEY_MODIFIERS(0);
    for (flag, win) in
        [(Modifiers::CTRL, MOD_CONTROL), (Modifiers::ALT, MOD_ALT), (Modifiers::SHIFT, MOD_SHIFT), (Modifiers::SUPER, MOD_WIN)]
    {
        if m.contains(flag) {
            out |= win;
        }
    }
    out
}

/// The app icon: resource 1 of the executable (`apps/laqta/laqta.rc`), or the stock application
/// icon when the executable has none.
fn app_icon() -> windows::Win32::UI::WindowsAndMessaging::HICON {
    // SAFETY: the module handle of the running executable; both loads only read resources.
    unsafe {
        GetModuleHandleW(None)
            .ok()
            .and_then(|m| LoadIconW(Some(HINSTANCE(m.0)), windows::core::PCWSTR(1 as _)).ok())
            .or_else(|| LoadIconW(None, IDI_APPLICATION).ok())
            .unwrap_or_default()
    }
}

/// Adds (or updates) the notification-area icon. No notification area means `Unsupported`.
fn notify(hwnd: HWND, tooltip: &str, add: bool) -> Result<(), PortError> {
    let mut data = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ID,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: WM_LAQTA_TRAY,
        hIcon: app_icon(),
        ..Default::default()
    };
    let tip: Vec<u16> = tooltip.encode_utf16().take(data.szTip.len() - 1).collect();
    data.szTip[..tip.len()].copy_from_slice(&tip);
    // SAFETY: `data` is fully initialised with cbSize set.
    let ok = unsafe { Shell_NotifyIconW(if add { NIM_ADD } else { NIM_MODIFY }, &data) };
    if ok.as_bool() {
        Ok(())
    } else if add {
        Err(PortError::Unsupported("no notification area (Explorer not running)".into()))
    } else {
        Err(PortError::Failed("could not update the tray icon".into()))
    }
}

fn show_menu(hwnd: HWND) {
    let Some((menu, callback)) = STATE.with(|s| s.borrow().tray.clone()) else { return };
    // SAFETY: the menu is created, shown and destroyed on this thread; no RefCell borrow is held
    // while TrackPopupMenu runs its modal loop (which re-enters the window procedure).
    let chosen = unsafe {
        let Ok(popup) = CreatePopupMenu() else { return };
        for (index, item) in menu.items.iter().enumerate() {
            let _ = AppendMenuW(popup, MF_STRING, index + 1, &HSTRING::from(item.label.as_str()));
        }
        if let Some(default) = menu.items.iter().position(|i| i.action == menu.default_action) {
            let _ = SetMenuDefaultItem(popup, default as u32 + 1, 0);
        }
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(popup, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY, pt.x, pt.y, None, hwnd, None);
        // Documented workaround so the menu closes properly on the next click elsewhere.
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(popup);
        cmd.0
    };
    if chosen > 0
        && let Some(item) = menu.items.get(chosen as usize - 1)
    {
        callback(item.action);
    }
}

#[cfg(test)]
mod tests {
    use super::refusal_text;

    #[test]
    fn a_hotkey_already_taken_says_so() {
        assert_eq!(refusal_text("Ctrl+Shift+S", 1409, "ignored"), "Ctrl+Shift+S is taken by another program");
    }

    #[test]
    fn other_refusals_carry_the_system_message() {
        assert_eq!(refusal_text("Ctrl+Shift+S", 5, "Access is denied."), "Windows refused Ctrl+Shift+S: Access is denied.");
    }
}
