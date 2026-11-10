//! macOS privacy permissions. A check fails with a message that names the permission and where to
//! turn it on, because that text is what the user sees in the toast. The first failing check of a
//! kind also asks macOS to show its own prompt; later checks only read the state, so a capture
//! never opens a prompt twice. Accessibility has no prompt without an options dictionary, so its
//! message alone sends the user to System Settings.

use std::sync::atomic::{AtomicBool, Ordering};

use laqta_ports::PortError;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
    fn CGPreflightListenEventAccess() -> bool;
    fn CGRequestListenEventAccess() -> bool;
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
}

fn denied(permission: &str) -> PortError {
    PortError::Failed(format!(
        "Laqta needs the {permission} permission. Turn it on in System Settings > Privacy & Security > {permission}, then restart Laqta."
    ))
}

fn check(permission: &str, granted: bool, asked: &AtomicBool, request: impl FnOnce() -> bool) -> Result<(), PortError> {
    if granted || (!asked.swap(true, Ordering::Relaxed) && request()) { Ok(()) } else { Err(denied(permission)) }
}

/// Screen Recording: screenshots, recording, window titles of other apps.
pub(crate) fn screen_recording() -> Result<(), PortError> {
    static ASKED: AtomicBool = AtomicBool::new(false);
    // SAFETY: plain C calls without arguments.
    check("Screen Recording", unsafe { CGPreflightScreenCaptureAccess() }, &ASKED, || unsafe { CGRequestScreenCaptureAccess() })
}

/// Input Monitoring: listening to mouse events of other apps (the pointer highlighter).
pub(crate) fn input_monitoring() -> Result<(), PortError> {
    static ASKED: AtomicBool = AtomicBool::new(false);
    // SAFETY: plain C calls without arguments.
    check("Input Monitoring", unsafe { CGPreflightListenEventAccess() }, &ASKED, || unsafe { CGRequestListenEventAccess() })
}

/// Accessibility: element hover and synthetic scrolling. Never prompts.
pub(crate) fn accessibility() -> Result<(), PortError> {
    // SAFETY: plain C call without arguments.
    if unsafe { AXIsProcessTrusted() } != 0 { Ok(()) } else { Err(denied("Accessibility")) }
}
