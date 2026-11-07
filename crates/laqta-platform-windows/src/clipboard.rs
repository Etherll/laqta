use std::borrow::Cow;

use laqta_domain::{Bitmap, PixelFormat};
use laqta_ports::{Clipboard, PortError};

#[derive(Debug)]
pub(crate) struct WindowsClipboard;

impl Clipboard for WindowsClipboard {
    fn set_image(&self, image: &Bitmap) -> Result<(), PortError> {
        let bytes: Cow<'_, [u8]> = match image.format() {
            PixelFormat::Rgba8 => Cow::Borrowed(image.pixels()),
            PixelFormat::Bgra8 => Cow::Owned(image.pixels().as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0], p[3]]).collect()),
        };
        let data = arboard::ImageData { width: image.width() as usize, height: image.height() as usize, bytes };
        arboard::Clipboard::new().and_then(|mut c| c.set_image(data)).map_err(|e| PortError::Failed(format!("clipboard: {e}")))
    }
}
