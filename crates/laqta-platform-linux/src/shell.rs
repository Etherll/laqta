use std::path::Path;
use std::process::{Command, Stdio};

use laqta_ports::{PortError, Shell};

/// Opens paths with `xdg-open`.
#[derive(Debug)]
pub struct XdgShell;

fn run_program(program: &Path, args: &[String]) -> laqta_ports::BoxFuture<'static, Result<i32, PortError>> {
    let mut cmd = Command::new(program);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    Box::pin(blocking::unblock(move || {
        let status = cmd.status().map_err(|e| PortError::Failed(format!("{}: {e}", cmd.get_program().to_string_lossy())))?;
        Ok(status.code().unwrap_or(-1))
    }))
}

impl Shell for XdgShell {
    fn run(&self, program: &Path, args: &[String], _hidden: bool) -> laqta_ports::BoxFuture<'_, Result<i32, PortError>> {
        run_program(program, args)
    }

    fn open_url(&self, url: &str) -> Result<(), PortError> {
        self.open_path(Path::new(url))
    }

    fn reveal(&self, path: &Path) -> Result<(), PortError> {
        // The FileManager1 D-Bus "ShowItems" call selects the file; fall back to opening its folder.
        let uri = format!("file://{}", path.display());
        let ok = Command::new("dbus-send")
            .args([
                "--session",
                "--dest=org.freedesktop.FileManager1",
                "--type=method_call",
                "/org/freedesktop/FileManager1",
                "org.freedesktop.FileManager1.ShowItems",
                &format!("array:string:{uri}"),
                "string:",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if ok { Ok(()) } else { self.open_path(path.parent().unwrap_or(path)) }
    }

    /// CUPS `lp` to the default printer.
    fn print(&self, path: &Path) -> Result<(), PortError> {
        let out = Command::new("lp")
            .arg("--")
            .arg(path)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| PortError::Unsupported(format!("lp (CUPS) is not available: {e}")))?;
        if out.status.success() { Ok(()) } else { Err(PortError::Failed(String::from_utf8_lossy(&out.stderr).trim().to_owned())) }
    }

    fn open_path(&self, path: &Path) -> Result<(), PortError> {
        let mut child = Command::new("xdg-open")
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| PortError::Unsupported(format!("xdg-open: {e}")))?;
        // Reap it in the background so no zombie is left.
        std::thread::Builder::new()
            .name("laqta-xdg-open".into())
            .spawn(move || {
                let _ = child.wait();
            })
            .map(|_| ())
            .map_err(|e| PortError::Failed(e.to_string()))
    }
}
