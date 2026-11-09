//! Window tools: borderless and top-most toggles for the active window, and the window inspector.

use std::collections::HashSet;
use std::sync::{Mutex, PoisonError};

use laqta_domain::region::HoverTarget;
use laqta_ports::AppError;

use crate::Ports;

/// Which windows the toggles have switched on. The platform cannot report a window's borderless or top-most state, so the
/// toggles remember what they set. Keep one value for as long as the toggles should remember.
#[derive(Debug, Default)]
pub struct WindowToggles {
    borderless: Mutex<HashSet<WindowKey>>,
    topmost: Mutex<HashSet<WindowKey>>,
}

/// A window's identity: its platform handle, or its title and process when the platform gives no handle.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum WindowKey {
    Handle(u64),
    Named { title: String, process: String },
}

impl WindowKey {
    fn of(window: &HoverTarget) -> WindowKey {
        if window.handle != 0 {
            WindowKey::Handle(window.handle)
        } else {
            WindowKey::Named { title: window.title.clone(), process: window.process.clone() }
        }
    }
}

/// Flips borderless on the active window and returns its new state. The state is recorded only when the platform accepts
/// the change.
pub async fn toggle_borderless_active(ports: &Ports, toggles: &WindowToggles) -> Result<bool, AppError> {
    let window = active_window(ports).await?;
    let key = WindowKey::of(&window);
    let on = !contains(&toggles.borderless, &key);
    ports.window_ops.set_borderless(&window, on)?;
    record(&toggles.borderless, key, on);
    Ok(on)
}

/// Flips top-most on the active window and returns its new state. The state is recorded only when the platform accepts
/// the change.
pub async fn toggle_topmost_active(ports: &Ports, toggles: &WindowToggles) -> Result<bool, AppError> {
    let window = active_window(ports).await?;
    let key = WindowKey::of(&window);
    let on = !contains(&toggles.topmost, &key);
    ports.window_ops.set_topmost(&window, on)?;
    record(&toggles.topmost, key, on);
    Ok(on)
}

/// The platform's properties of the active window, as `(name, value)` rows.
pub async fn inspect_active(ports: &Ports) -> Result<Vec<(String, String)>, AppError> {
    let window = active_window(ports).await?;
    Ok(ports.window_ops.inspect(&window)?)
}

async fn active_window(ports: &Ports) -> Result<HoverTarget, AppError> {
    ports.hover.active_window().await?.ok_or_else(|| AppError::Unsupported("no active window to act on".to_owned()))
}

fn contains(set: &Mutex<HashSet<WindowKey>>, key: &WindowKey) -> bool {
    set.lock().unwrap_or_else(PoisonError::into_inner).contains(key)
}

fn record(set: &Mutex<HashSet<WindowKey>>, key: WindowKey, on: bool) {
    let mut set = set.lock().unwrap_or_else(PoisonError::into_inner);
    if on {
        set.insert(key);
    } else {
        set.remove(&key);
    }
}
