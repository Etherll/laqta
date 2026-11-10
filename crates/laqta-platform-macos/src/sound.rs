//! Per-event sounds.

use std::path::Path;

use laqta_ports::{PortError, Sound};

pub fn play_sound(_sound: Sound, _custom: Option<&Path>) -> Result<(), PortError> {
    Err(PortError::Unsupported("sounds on macOS".into()))
}
