//! GPUI views and their GPUI-free presenters (ADR 017).
//!
//! The views implement the `Overlay`, `Notifier` and `ToolHost` ports through [`UiHandle`]; `main`
//! runs [`run`] on the main thread.

pub mod presenters;

mod actions_toolbar;
mod editor_window;
mod history_window;
mod main_window;
mod overlay;
mod prompt_window;
mod runtime;
mod settings_window;
mod theme;
mod toast;
mod tray_popup;
mod url_window;
mod widgets;

pub use runtime::{UiHandle, UiReceiver, UiServices, run};
