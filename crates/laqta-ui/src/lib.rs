//! GPUI views and their GPUI-free presenters (ADR 017).
//!
//! The views implement the `Overlay`, `Notifier` and `ToolHost` ports through [`UiHandle`]; `main`
//! runs [`run`] on the main thread.

pub mod i18n;
pub mod presenters;

/// The app id normal windows carry on Wayland (and as their X11 class). Desktops match it to
/// the desktop entry of the same name, which is how the taskbar finds the app icon.
pub(crate) const APP_ID: &str = "io.github.etherll.Laqta";

mod actions_toolbar;
mod assets;
mod chrome;
mod color_window;
mod editor_window;
mod effects_window;
mod gpu_image;
mod history_window;
mod main_window;
mod overlay;
mod overlay_tools;
mod perf;
mod photo_window;
mod pin_window;
mod prompt_window;
mod recording_bar;
mod remote_window;
mod runtime;
mod settings_window;
mod text_window;
mod theme;
mod thumbnails;
mod toast;
mod tools_window;
mod tray_popup;
mod url_window;
mod vanish;
mod widgets;

#[cfg(test)]
mod headless_tests;

pub use runtime::{HotkeyReport, ModelProgress, ModelRow, ModelService, UiHandle, UiReceiver, UiServices, run};
