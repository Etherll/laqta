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
        if code > 32 { Ok(()) } else { Err(PortError::Failed(format!("could not open {} (code {code})", path.display()))) }
    }

    fn open_url(&self, url: &str) -> Result<(), PortError> {
        self.open_path(Path::new(url))
    }

    /// `explorer.exe /select,<path>` opens the folder with the file selected.
    fn reveal(&self, path: &Path) -> Result<(), PortError> {
        std::process::Command::new("explorer.exe")
            .arg(format!("/select,{}", path.display()))
            .spawn()
            .map(|_| ())
            .map_err(|e| PortError::Failed(format!("explorer: {e}")))
    }

    fn run(&self, program: &Path, args: &[String], hidden: bool) -> laqta_ports::BoxFuture<'_, Result<i32, PortError>> {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut cmd = std::process::Command::new(program);
        cmd.args(args);
        if hidden {
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        Box::pin(blocking::unblock(move || {
            let status = cmd.status().map_err(|e| PortError::Failed(format!("{}: {e}", cmd.get_program().to_string_lossy())))?;
            Ok(status.code().unwrap_or(-1))
        }))
    }
}
