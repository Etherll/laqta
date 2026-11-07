use std::path::Path;
use std::process::{Command, Stdio};

use laqta_ports::{PortError, Shell};

/// Opens paths with `xdg-open`.
#[derive(Debug)]
pub struct XdgShell;

impl Shell for XdgShell {
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
