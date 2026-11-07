use std::path::Path;
use std::sync::Arc;

use laqta_domain::{Action, Bitmap, KeyChord};

use crate::PortError;

pub trait Clipboard: Send + Sync {
    fn set_image(&self, image: &Bitmap) -> Result<(), PortError>;
    fn set_text(&self, text: &str) -> Result<(), PortError> {
        let _ = text;
        Err(PortError::Unsupported("clipboard text".into()))
    }
    /// Puts files on the clipboard (a file list, as a file manager copy does).
    fn set_files(&self, paths: &[std::path::PathBuf]) -> Result<(), PortError> {
        let _ = paths;
        Err(PortError::Unsupported("clipboard files".into()))
    }
}

pub trait Shell: Send + Sync {
    fn open_path(&self, path: &Path) -> Result<(), PortError>;
    /// Shows the file selected in the file manager; falls back to opening its folder.
    fn reveal(&self, path: &Path) -> Result<(), PortError> {
        match path.parent() {
            Some(dir) => self.open_path(dir),
            None => self.open_path(path),
        }
    }
    fn open_url(&self, url: &str) -> Result<(), PortError> {
        let _ = url;
        Err(PortError::Unsupported("open url".into()))
    }
}

pub type ActionCallback = Arc<dyn Fn(Action) + Send + Sync>;

pub trait HotkeyRegistrar: Send + Sync {
    fn register(&self, bindings: &[(KeyChord, Action)], on_fire: ActionCallback) -> Vec<(KeyChord, Result<(), PortError>)>;
    /// Drops every registration so `register` can be called again with new bindings.
    fn unregister_all(&self) {}
}

#[derive(Clone, Debug)]
pub struct TrayItem {
    pub label: String,
    pub action: Action,
}

#[derive(Clone, Debug)]
pub struct TrayMenu {
    pub tooltip: String,
    pub default_action: Action,
    pub items: Vec<TrayItem>,
}

pub trait Tray: Send + Sync {
    fn show(&self, menu: TrayMenu, on_select: ActionCallback) -> Result<(), PortError>;
}
