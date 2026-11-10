//! Start at login (`Autostart`) through `SMAppService`, which registers the running app bundle.
//! The `executable` argument is not used: macOS starts the bundle, not a path.

use std::path::Path;

use laqta_ports::{Autostart, PortError};
use objc2_foundation::{NSBundle, NSError};
use objc2_service_management::{SMAppService, SMAppServiceStatus};

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

#[derive(Debug, Default)]
pub struct MacAutostart;

impl Autostart for MacAutostart {
    fn is_enabled(&self) -> Result<bool, PortError> {
        require_bundle()?;
        // SAFETY: `mainAppService` and `status` take no arguments and have no preconditions.
        let status = unsafe { SMAppService::mainAppService().status() };
        Ok(status == SMAppServiceStatus::Enabled)
    }

    fn set_enabled(&self, enabled: bool, _executable: &Path) -> Result<(), PortError> {
        require_bundle()?;
        // SAFETY: as in `is_enabled`.
        let service = unsafe { SMAppService::mainAppService() };
        // SAFETY: `status` has no preconditions.
        let status = unsafe { service.status() };
        if enabled {
            // Registering twice is an error, so an enabled app is left alone.
            if status == SMAppServiceStatus::Enabled {
                return Ok(());
            }
            // SAFETY: `registerAndReturnError` has no preconditions; the error is returned, not written to a pointer.
            unsafe { service.registerAndReturnError() }.map_err(|e| failed("turn on", &e))
        } else {
            // Unregistering an app that is not registered is an error too.
            if matches!(status, SMAppServiceStatus::NotRegistered | SMAppServiceStatus::NotFound) {
                return Ok(());
            }
            // SAFETY: as for `registerAndReturnError`.
            unsafe { service.unregisterAndReturnError() }.map_err(|e| failed("turn off", &e))
        }
    }
}
