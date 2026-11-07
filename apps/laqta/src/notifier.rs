//! Toasts go to the GPUI UI; sounds go to the platform's player.

use std::path::Path;

use laqta_ports::{Notifier, PortError, Sound, Toast};
use laqta_ui::UiHandle;

#[derive(Debug, Clone)]
pub struct AppNotifier {
    pub ui: UiHandle,
}

impl Notifier for AppNotifier {
    fn toast(&self, toast: Toast) -> Result<(), PortError> {
        self.ui.toast(toast)
    }

    fn sound(&self, sound: Sound, custom: Option<&Path>) -> Result<(), PortError> {
        play(sound, custom)
    }
}

#[cfg(target_os = "linux")]
fn play(sound: Sound, custom: Option<&Path>) -> Result<(), PortError> {
    laqta_platform_linux::play_sound(sound, custom)
}

#[cfg(windows)]
fn play(sound: Sound, custom: Option<&Path>) -> Result<(), PortError> {
    laqta_platform_windows::play_sound(sound, custom)
}

#[cfg(not(any(target_os = "linux", windows)))]
fn play(_sound: Sound, _custom: Option<&Path>) -> Result<(), PortError> {
    Err(PortError::Unsupported("sounds".into()))
}
