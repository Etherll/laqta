//! GPUI views and their GPUI-free presenters (ADR 017).
//!
//! The views implement the `Overlay`, `Notifier` and `ToolHost` ports through [`UiHandle`]; `main`
//! runs [`run`] on the main thread.

pub mod presenters;

mod history_window;
mod main_window;
mod overlay;
mod runtime;
mod settings_window;
mod theme;
mod toast;
mod widgets;

pub use runtime::{UiHandle, UiReceiver, UiServices, run};
