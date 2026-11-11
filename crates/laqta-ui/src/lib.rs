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
mod close_guard;
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
mod popup;
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

pub use runtime::{
    HotkeyReport, LogicalRectHook, ModelProgress, ModelRow, ModelService, NativeWindow, UiHandle, UiReceiver, UiServices, run,
};

/// The OS this build runs on, for the parts of the core that differ by platform.
pub(crate) fn host_os() -> laqta_domain::record::Os {
    use laqta_domain::record::Os;
    if cfg!(windows) {
        Os::Windows
    } else if cfg!(target_os = "macos") {
        Os::MacOs
    } else {
        Os::Linux
    }
}
