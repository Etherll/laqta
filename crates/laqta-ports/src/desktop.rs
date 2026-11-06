use std::path::Path;
use std::sync::Arc;

use laqta_domain::{Action, Bitmap, KeyChord};

use crate::PortError;

pub trait Clipboard: Send + Sync {
    fn set_image(&self, image: &Bitmap) -> Result<(), PortError>;
}

pub trait Shell: Send + Sync {
    fn open_path(&self, path: &Path) -> Result<(), PortError>;
}

pub type ActionCallback = Arc<dyn Fn(Action) + Send + Sync>;

pub trait HotkeyRegistrar: Send + Sync {
    fn register(&self, bindings: &[(KeyChord, Action)], on_fire: ActionCallback) -> Vec<(KeyChord, Result<(), PortError>)>;
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
