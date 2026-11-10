//! The pasteboard (`Clipboard`).

use laqta_domain::Bitmap;
use laqta_ports::{Clipboard, PortError};

#[derive(Debug, Default)]
pub struct MacClipboard;

impl Clipboard for MacClipboard {
    fn set_image(&self, _image: &Bitmap) -> Result<(), PortError> {
        Err(PortError::Unsupported("the clipboard on macOS".into()))
    }
}
