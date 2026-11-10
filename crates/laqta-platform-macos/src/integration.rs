//! Finder and browser integration (`ShellIntegration`). The Finder entries ("Open With", the
//! Services menu) are declared by the app's Info.plist, so there is nothing to install; the
//! right-click menu would need a Finder Sync extension and is `Unsupported`. Browser manifests are
//! files under `~/Library/Application Support`, written here.

use std::fs;
use std::path::Path;

use laqta_ports::{PortError, ShellIntegration};

use crate::autostart::running_from_bundle;
use crate::manifest;

const NO_FINDER_MENU: &str = "Finder menu entries come with the app; drag a file onto Laqta or use Open With";

#[derive(Debug, Default)]
pub struct MacIntegration;

impl ShellIntegration for MacIntegration {
    fn is_installed(&self) -> Result<bool, PortError> {
        Ok(running_from_bundle())
    }

    fn install(&self, _executable: &Path) -> Result<(), PortError> {
        Err(PortError::Unsupported(NO_FINDER_MENU.into()))
    }

    fn uninstall(&self) -> Result<(), PortError> {
        Err(PortError::Unsupported(NO_FINDER_MENU.into()))
    }

    fn install_native_messaging(&self, nmh: &Path) -> Result<(), PortError> {
        if !nmh.is_absolute() {
            return Err(PortError::Failed(format!("the native host path must be absolute: {}", nmh.display())));
        }
        let home = std::env::home_dir()
            .filter(|home| home.is_absolute())
            .ok_or_else(|| PortError::Failed("could not find the home folder".into()))?;
        let files = manifest::manifest_files(&home, nmh, Path::is_dir);
        if files.is_empty() {
            return Err(PortError::Failed(
                "no supported browser found; open Chrome, Edge, Brave, Vivaldi, Chromium or Firefox once, then try again".into(),
            ));
        }
        for (path, text) in files {
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir).map_err(|e| PortError::Failed(format!("could not create {}: {e}", dir.display())))?;
            }
            fs::write(&path, text).map_err(|e| PortError::Failed(format!("could not write {}: {e}", path.display())))?;
            tracing::debug!(path = %path.display(), "wrote native messaging manifest");
        }
        Ok(())
    }
}
