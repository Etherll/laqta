//! The pointer highlighter (`PointerHighlighter`).

use laqta_ports::{PointerHighlighter, PortError};

#[derive(Debug, Default)]
pub struct MacHighlighter;

impl PointerHighlighter for MacHighlighter {
    fn set_enabled(&self, _on: bool) -> Result<(), PortError> {
        Err(PortError::Unsupported("the pointer highlighter on macOS".into()))
    }

    fn is_enabled(&self) -> bool {
        false
    }
}
