//! Synthetic scrolling for scrolling capture (`InputSynth`).

use laqta_domain::capture::ScrollMethod;
use laqta_ports::{InputSynth, PortError};

#[derive(Debug, Default)]
pub struct MacInput;

impl InputSynth for MacInput {
    fn scroll(&self, _x: i32, _y: i32, _method: ScrollMethod, _amount: u32) -> Result<(), PortError> {
        Err(PortError::Unsupported("scrolling other windows on macOS".into()))
    }
}
