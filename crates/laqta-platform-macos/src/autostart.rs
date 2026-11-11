//! Start at login (`Autostart`) through `SMAppService`. It registers the launch agent bundled in
//! `Contents/Library/LaunchAgents`, which starts Laqta with `-silent`. The main-app login item
//! that earlier versions registered starts it with no arguments, so it is read as "on" and removed.
//! The `executable` argument is not used: macOS starts the bundle, not a path.

use std::path::Path;

use laqta_ports::{Autostart, PortError};
use objc2::rc::Retained;
use objc2_foundation::{NSBundle, NSError, NSString};
use objc2_service_management::{SMAppService, SMAppServiceStatus};

/// The launch agent's file name in `Contents/Library/LaunchAgents` (from `packaging/macos`).
pub(crate) const LOGIN_AGENT: &str = "io.github.etherll.Laqta.login.plist";

/// True when Laqta runs from an `.app` bundle. A bare binary has no bundle identifier, and
/// `SMAppService` and the Info.plist entries only work for a bundle.
pub(crate) fn running_from_bundle() -> bool {
    NSBundle::mainBundle().bundleIdentifier().is_some()
}

fn require_bundle() -> Result<(), PortError> {
    if running_from_bundle() { Ok(()) } else { Err(PortError::Unsupported("start at login needs Laqta installed as an app".into())) }
}

fn failed(action: &str, error: &NSError) -> PortError {
    PortError::Failed(format!("could not {action} start at login: {} (code {})", error.localizedDescription(), error.code()))
}

fn agent() -> Retained<SMAppService> {
    // SAFETY: `agentServiceWithPlistName` takes a valid NSString and has no other preconditions.
    unsafe { SMAppService::agentServiceWithPlistName(&NSString::from_str(LOGIN_AGENT)) }
}

fn legacy_main_app() -> Retained<SMAppService> {
    // SAFETY: `mainAppService` takes no arguments and has no preconditions.
    unsafe { SMAppService::mainAppService() }
}

fn is_on(service: &SMAppService) -> bool {
    // SAFETY: `status` has no preconditions.
    let status = unsafe { service.status() };
    status == SMAppServiceStatus::Enabled
}

fn turn_on(service: &SMAppService) -> Result<(), PortError> {
    // Registering twice is an error, so an enabled service is left alone.
    if is_on(service) {
        return Ok(());
    }
    // SAFETY: `registerAndReturnError` has no preconditions; the error is returned, not written to a pointer.
    unsafe { service.registerAndReturnError() }.map_err(|e| failed("turn on", &e))
}

fn turn_off(service: &SMAppService) -> Result<(), PortError> {
    // Unregistering a service that is not registered is an error too.
    // SAFETY: `status` has no preconditions.
    if matches!(unsafe { service.status() }, SMAppServiceStatus::NotRegistered | SMAppServiceStatus::NotFound) {
        return Ok(());
    }
    // SAFETY: as for `registerAndReturnError`.
    unsafe { service.unregisterAndReturnError() }.map_err(|e| failed("turn off", &e))
}

#[derive(Debug, Default)]
pub struct MacAutostart;

impl Autostart for MacAutostart {
    fn is_enabled(&self) -> Result<bool, PortError> {
        require_bundle()?;
        Ok(is_on(&agent()) || is_on(&legacy_main_app()))
    }

    fn set_enabled(&self, enabled: bool, _executable: &Path) -> Result<(), PortError> {
        require_bundle()?;
        if enabled {
            turn_on(&agent())?;
        } else {
            turn_off(&agent())?;
        }
        // The old item opens the main window at login, so it goes either way.
        turn_off(&legacy_main_app())
    }

    fn migrate(&self, executable: &Path) -> Result<(), PortError> {
        if running_from_bundle() && is_on(&legacy_main_app()) { self.set_enabled(true, executable) } else { Ok(()) }
    }
}

#[cfg(test)]
mod tests {
    use super::LOGIN_AGENT;

    #[test]
    fn the_bundled_agent_starts_laqta_silently() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../packaging/macos/");
        let plist = std::fs::read_to_string(format!("{path}{LOGIN_AGENT}")).expect("the agent plist is in packaging/macos");
        assert!(plist.contains("<string>Contents/MacOS/laqta</string>"), "{plist}");
        assert!(plist.contains("<string>-silent</string>"), "{plist}");
        let script = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts/macos-bundle.sh")).expect("bundle script");
        assert!(script.contains(&format!("Contents/Library/LaunchAgents/{LOGIN_AGENT}")), "the bundle script installs the agent");
    }
}
