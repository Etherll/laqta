//! macOS privacy permissions. Each check returns `PortError::PermissionDenied` when the user has
//! not granted the permission. The first failing check of a kind also asks macOS to show its
//! prompt (or, for Accessibility, leaves it to the UI to open System Settings); later checks only
//! read the state, so a capture never opens a prompt twice.

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

fn check(granted: bool, asked: &AtomicBool, request: impl FnOnce() -> bool) -> Result<(), PortError> {
    if granted || (!asked.swap(true, Ordering::Relaxed) && request()) { Ok(()) } else { Err(PortError::PermissionDenied) }
}

/// Screen Recording: screenshots, recording, window titles of other apps.
pub(crate) fn screen_recording() -> Result<(), PortError> {
    static ASKED: AtomicBool = AtomicBool::new(false);
    // SAFETY: plain C calls without arguments.
    check(unsafe { CGPreflightScreenCaptureAccess() }, &ASKED, || unsafe { CGRequestScreenCaptureAccess() })
}

/// Input Monitoring: listening to mouse events of other apps (the pointer highlighter).
pub(crate) fn input_monitoring() -> Result<(), PortError> {
    static ASKED: AtomicBool = AtomicBool::new(false);
    // SAFETY: plain C calls without arguments.
    check(unsafe { CGPreflightListenEventAccess() }, &ASKED, || unsafe { CGRequestListenEventAccess() })
}

/// Accessibility: window tools, element hover, synthetic scrolling. Never prompts.
pub(crate) fn accessibility() -> Result<(), PortError> {
    // SAFETY: plain C call without arguments.
    if unsafe { AXIsProcessTrusted() } != 0 { Ok(()) } else { Err(PortError::PermissionDenied) }
}
