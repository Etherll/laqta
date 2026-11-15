//! Notices when a program running as administrator takes the focus while Laqta runs without those
//! rights. Windows keeps such a program out of Laqta's reach (its shortcuts and captures don't get
//! through), so the app offers to restart Laqta as administrator.

/// Programs that run as administrator on their own and that people open often for a moment (Task
/// Manager, the UAC prompt, management consoles). An offer for each would only be noise.
#[cfg(any(windows, test))]
const QUIET: [&str; 3] = ["taskmgr", "consent", "mmc"];

/// Whether an offer is worth making for the program `exe_stem` (its file name without `.exe`).
#[cfg(any(windows, test))]
fn worth_offering(exe_stem: &str) -> bool {
    !exe_stem.is_empty() && !QUIET.iter().any(|quiet| exe_stem.eq_ignore_ascii_case(quiet))
}

#[cfg(windows)]
pub use imp::watch;

#[cfg(windows)]
mod imp {
    use std::sync::{Arc, Mutex, PoisonError};

    use windows::Win32::Foundation::{CloseHandle, HWND};
    use windows::Win32::System::Threading::{
        GetCurrentProcessId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
    use windows::Win32::UI::WindowsAndMessaging::{
        EVENT_SYSTEM_FOREGROUND, GetMessageW, GetWindowThreadProcessId, MSG, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
    };
    use windows::core::PWSTR;

    /// Told the file name (without `.exe`) of each program running as administrator that takes the focus.
    type OnElevated = Arc<dyn Fn(String) + Send + Sync>;

    static SINK: Mutex<Option<OnElevated>> = Mutex::new(None);

    /// Calls `on_elevated` whenever a program running as administrator takes the focus, from a thread
    /// of its own that lives as long as the process. Call it once, and only when Laqta itself runs
    /// without administrator rights.
    pub fn watch(on_elevated: OnElevated) {
        *SINK.lock().unwrap_or_else(PoisonError::into_inner) = Some(on_elevated);
        let spawned = std::thread::Builder::new().name("laqta-elevated-watch".into()).spawn(|| {
            // SAFETY: an out-of-context hook delivered to this thread's message loop below.
            let hook = unsafe {
                SetWinEventHook(
                    EVENT_SYSTEM_FOREGROUND,
                    EVENT_SYSTEM_FOREGROUND,
                    None,
                    Some(on_foreground),
                    0,
                    0,
                    WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
                )
            };
            if hook.is_invalid() {
                tracing::warn!("cannot watch for programs running as administrator");
                return;
            }
            let mut msg = MSG::default();
            // SAFETY: a plain message loop; the hook's callbacks arrive through it.
            while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {}
        });
        if let Err(e) = spawned {
            tracing::warn!("cannot watch for programs running as administrator: {e}");
        }
    }

    unsafe extern "system" fn on_foreground(_: HWINEVENTHOOK, _: u32, hwnd: HWND, _: i32, _: i32, _: u32, _: u32) {
        let mut pid = 0u32;
        // SAFETY: a plain query; a window that is gone leaves `pid` at 0.
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        // SAFETY: a plain query of this process's id.
        if pid == 0 || pid == unsafe { GetCurrentProcessId() } || !crate::elevation::process_is_elevated(pid) {
            return;
        }
        let Some(name) = exe_stem(pid).filter(|name| super::worth_offering(name)) else { return };
        let sink = SINK.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if let Some(sink) = sink {
            sink(name);
        }
    }

    /// The file name of process `pid`'s program, without `.exe`.
    fn exe_stem(pid: u32) -> Option<String> {
        // SAFETY: the handle is closed before returning; the buffer outlives the call.
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buffer = [0u16; 1024];
            let mut len = buffer.len() as u32;
            let read = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut len);
            let _ = CloseHandle(process);
            read.ok()?;
            let path = String::from_utf16_lossy(&buffer[..len as usize]);
            std::path::Path::new(&path).file_stem().map(|stem| stem.to_string_lossy().into_owned())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::worth_offering;

    #[test]
    fn programs_that_run_as_administrator_on_their_own_get_no_offer() {
        assert!(!worth_offering("Taskmgr"));
        assert!(!worth_offering("consent"));
        assert!(!worth_offering(""));
        assert!(worth_offering("Game"));
        assert!(worth_offering("devenv"));
    }
}
