//! Opening and revealing files and URLs, and running programs (`Shell`).

use std::path::Path;
use std::process::{Command, Stdio};

use laqta_ports::{BoxFuture, PortError, Shell};
use objc2::rc::Retained;
use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSArray, NSString, NSURL};

use crate::main_thread::on_main;

#[derive(Debug, Default)]
pub struct MacShell;

impl Shell for MacShell {
    fn open_path(&self, path: &Path) -> Result<(), PortError> {
        open(&path.display().to_string(), || file_url(path))
    }

    fn open_url(&self, url: &str) -> Result<(), PortError> {
        open(url, || NSURL::URLWithString(&NSString::from_str(url)))
    }

    /// Finder opens the parent folder with the file selected. A path that is gone opens the folder alone.
    fn reveal(&self, path: &Path) -> Result<(), PortError> {
        if !path.exists() {
            return self.open_path(path.parent().unwrap_or(path));
        }
        let revealed = on_main(|_| {
            file_url(path).map(|url| NSWorkspace::sharedWorkspace().activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&[url])))
        });
        revealed.ok_or_else(|| PortError::Failed(format!("could not reveal {}", path.display())))
    }

    /// NSWorkspace has no print call, and shelling out to `lpr` is not wanted.
    fn print(&self, _path: &Path) -> Result<(), PortError> {
        Err(PortError::Unsupported("printing".into()))
    }

    fn run(&self, program: &Path, args: &[String], _hidden: bool) -> BoxFuture<'_, Result<i32, PortError>> {
        let mut cmd = Command::new(program);
        cmd.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        Box::pin(blocking::unblock(move || {
            let status = cmd.status().map_err(|e| PortError::Failed(format!("{}: {e}", cmd.get_program().to_string_lossy())))?;
            Ok(status.code().unwrap_or(-1))
        }))
    }
}

/// Opens the URL `make_url` builds with its default app. `what` names it in errors.
fn open(what: &str, make_url: impl FnOnce() -> Option<Retained<NSURL>> + Send) -> Result<(), PortError> {
    match on_main(|_| make_url().map(|url| NSWorkspace::sharedWorkspace().openURL(&url))) {
        Some(true) => Ok(()),
        Some(false) => Err(PortError::Failed(format!("could not open {what}"))),
        None => Err(PortError::Failed(format!("not a valid path or URL: {what}"))),
    }
}

/// `None` when the path is not valid UTF-8, which `NSString` cannot hold.
fn file_url(path: &Path) -> Option<Retained<NSURL>> {
    path.to_str().map(|p| NSURL::fileURLWithPath(&NSString::from_str(p)))
}
