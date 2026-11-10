//! Opening and revealing files (`Shell`).

use std::path::Path;

use laqta_ports::{PortError, Shell};

#[derive(Debug, Default)]
pub struct MacShell;

impl Shell for MacShell {
    fn open_path(&self, _path: &Path) -> Result<(), PortError> {
        Err(PortError::Unsupported("opening files on macOS".into()))
    }
}
