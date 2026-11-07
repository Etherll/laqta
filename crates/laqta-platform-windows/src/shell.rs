use std::path::Path;

use laqta_ports::{PortError, Shell};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, PCWSTR, w};

#[derive(Debug)]
pub(crate) struct WindowsShell;

impl Shell for WindowsShell {
    fn open_path(&self, path: &Path) -> Result<(), PortError> {
        let file = HSTRING::from(path.as_os_str());
        // SAFETY: all strings outlive the call.
        let result = unsafe { ShellExecuteW(None, w!("open"), &file, PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL) };
        // ShellExecuteW returns a value greater than 32 on success.
        let code = result.0 as isize;
        if code > 32 {
            Ok(())
        } else {
            Err(PortError::Failed(format!("could not open {} (code {code})", path.display())))
        }
    }
}
