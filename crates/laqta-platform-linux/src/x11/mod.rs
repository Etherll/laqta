pub mod capture;
pub mod hotkeys;
pub mod hover;
pub mod windows;

use std::fmt::Display;

use laqta_ports::PortError;

/// Maps an X11 failure to the port error every X11 adapter returns.
pub(crate) fn fail(e: impl Display) -> PortError {
    PortError::Failed(format!("x11: {e}"))
}
