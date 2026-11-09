//! Run at login: the `Laqta` value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
//! The value text is pure and tested on every target; the registry calls are Windows-only.

use std::path::Path;

#[cfg(windows)]
use laqta_ports::{Autostart, PortError};

#[cfg(windows)]
use crate::integration::OpenKey;

#[cfg(windows)]
const VALUE_NAME: &str = "Laqta";

/// The Run value's data for `executable`: the quoted path and `-silent`, so the start stays quiet.
pub(crate) fn run_command(executable: &Path) -> String {
    format!("\"{}\" -silent", executable.display())
}

/// The Run key under `HKCU`, and the value Laqta keeps there.
#[cfg(windows)]
#[derive(Debug)]
pub struct WindowsAutostart {
    run_key: String,
}

#[cfg(windows)]
impl Default for WindowsAutostart {
    fn default() -> Self {
        Self { run_key: r"Software\Microsoft\Windows\CurrentVersion\Run".to_owned() }
    }
}

#[cfg(windows)]
impl Autostart for WindowsAutostart {
    fn is_enabled(&self) -> Result<bool, PortError> {
        match OpenKey::open(&self.run_key)? {
            Some(key) => key.has_value(VALUE_NAME),
            None => Ok(false),
        }
    }

    fn set_enabled(&self, enabled: bool, executable: &Path) -> Result<(), PortError> {
        if enabled {
            OpenKey::create(&self.run_key)?.set_string(Some(VALUE_NAME), &run_command(executable))
        } else {
            match OpenKey::open(&self.run_key)? {
                Some(key) => key.delete_value(VALUE_NAME),
                None => Ok(()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::run_command;

    #[test]
    fn run_value_quotes_the_path_and_starts_silently() {
        assert_eq!(run_command(Path::new(r"C:\Program Files\Laqta\laqta.exe")), r#""C:\Program Files\Laqta\laqta.exe" -silent"#);
    }
}
