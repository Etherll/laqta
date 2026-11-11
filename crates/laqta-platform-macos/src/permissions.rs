//! macOS privacy permissions. The onboarding window reads them with [`granted`] and asks for one
//! with [`request`]. A check made when a feature runs fails with a message that names the
//! permission and where to turn it on, because that text is what the user sees in the toast. The
//! first failing check of a kind also asks macOS to show its own prompt, unless onboarding already
//! asked; later checks only read the state, so a capture never opens a prompt twice.

use std::sync::atomic::{AtomicBool, Ordering};

use laqta_ports::{Permission, PortError};
use objc2_foundation::{NSDictionary, NSNumber, NSString};

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
    fn AXIsProcessTrustedWithOptions(options: *const NSDictionary<NSString, NSNumber>) -> u8;
}

static ASKED_SCREEN: AtomicBool = AtomicBool::new(false);
static ASKED_INPUT: AtomicBool = AtomicBool::new(false);

/// The name System Settings shows for `permission`.
fn name(permission: Permission) -> &'static str {
    match permission {
        Permission::ScreenRecording => "Screen Recording",
        Permission::Accessibility => "Accessibility",
        Permission::InputMonitoring => "Input Monitoring",
    }
}

/// The System Settings pane that lists the apps holding `permission`.
fn settings_url(permission: Permission) -> &'static str {
    match permission {
        Permission::ScreenRecording => "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
        Permission::Accessibility => "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
        Permission::InputMonitoring => "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent",
    }
}

/// Whether Laqta holds `permission` now. Never prompts. Screen Recording reads as granted only
/// after Laqta restarts, as macOS applies it to new processes.
pub fn granted(permission: Permission) -> bool {
    // SAFETY: plain C calls without arguments.
    unsafe {
        match permission {
            Permission::ScreenRecording => CGPreflightScreenCaptureAccess(),
            Permission::Accessibility => AXIsProcessTrusted() != 0,
            Permission::InputMonitoring => CGPreflightListenEventAccess(),
        }
    }
}

/// Asks for `permission`: macOS adds Laqta to the permission's list and shows its own prompt the
/// first time, then System Settings opens at that list, where the user turns Laqta on.
pub fn request(permission: Permission) {
    // SAFETY: plain C calls; the options dictionary lives across the call.
    unsafe {
        match permission {
            Permission::ScreenRecording => {
                ASKED_SCREEN.store(true, Ordering::Relaxed);
                CGRequestScreenCaptureAccess();
            }
            Permission::Accessibility => {
                // `kAXTrustedCheckOptionPrompt`'s value; the prompt is what lists Laqta.
                let key = NSString::from_str("AXTrustedCheckOptionPrompt");
                let options = NSDictionary::from_slices(&[&*key], &[&*NSNumber::new_bool(true)]);
                AXIsProcessTrustedWithOptions(&*options);
            }
            Permission::InputMonitoring => {
                ASKED_INPUT.store(true, Ordering::Relaxed);
                CGRequestListenEventAccess();
            }
        }
    }
    let url = settings_url(permission);
    if let Err(error) = laqta_ports::Shell::open_url(&crate::shell::MacShell, url) {
        tracing::warn!(%error, "could not open the {} settings", name(permission));
    }
}

fn denied(permission: Permission) -> PortError {
    let permission = name(permission);
    PortError::Failed(format!(
        "Laqta needs the {permission} permission. Turn it on in System Settings > Privacy & Security > {permission}, then restart Laqta."
    ))
}

fn check(permission: Permission, asked: &AtomicBool, request: impl FnOnce() -> bool) -> Result<(), PortError> {
    if granted(permission) || (!asked.swap(true, Ordering::Relaxed) && request()) { Ok(()) } else { Err(denied(permission)) }
}

/// Screen Recording: screenshots, recording, window titles of other apps.
pub(crate) fn screen_recording() -> Result<(), PortError> {
    // SAFETY: plain C call without arguments.
    check(Permission::ScreenRecording, &ASKED_SCREEN, || unsafe { CGRequestScreenCaptureAccess() })
}

/// Input Monitoring: listening to mouse events of other apps (the pointer highlighter).
pub(crate) fn input_monitoring() -> Result<(), PortError> {
    // SAFETY: plain C call without arguments.
    check(Permission::InputMonitoring, &ASKED_INPUT, || unsafe { CGRequestListenEventAccess() })
}

/// Accessibility: element hover and synthetic scrolling. Never prompts.
pub(crate) fn accessibility() -> Result<(), PortError> {
    if granted(Permission::Accessibility) { Ok(()) } else { Err(denied(Permission::Accessibility)) }
}
