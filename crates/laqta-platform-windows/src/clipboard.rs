use std::borrow::Cow;

use laqta_domain::{Bitmap, PixelFormat};
use laqta_ports::{Clipboard, ClipboardContent, PortError};

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

    fn set_text(&self, text: &str) -> Result<(), PortError> {
        arboard::Clipboard::new().and_then(|mut c| c.set_text(text.to_string())).map_err(|e| PortError::Failed(format!("clipboard: {e}")))
    }

    fn set_files(&self, paths: &[std::path::PathBuf]) -> Result<(), PortError> {
        arboard::Clipboard::new().and_then(|mut c| c.set().file_list(paths)).map_err(|e| PortError::Failed(format!("clipboard: {e}")))
    }

    fn get(&self) -> Result<Option<ClipboardContent>, PortError> {
        let mut cb = arboard::Clipboard::new().map_err(|e| PortError::Failed(format!("clipboard: {e}")))?;
        read_clipboard(&mut cb)
    }
}

fn read_clipboard(cb: &mut arboard::Clipboard) -> Result<Option<ClipboardContent>, PortError> {
    if let Ok(img) = cb.get_image() {
        let bmp = Bitmap::new(img.width as u32, img.height as u32, PixelFormat::Rgba8, std::sync::Arc::from(img.bytes.into_owned()))
            .map_err(|e| PortError::Failed(format!("clipboard image: {e}")))?;
        return Ok(Some(ClipboardContent::Image(bmp)));
    }
    if let Ok(files) = cb.get().file_list()
        && !files.is_empty()
    {
        return Ok(Some(ClipboardContent::Files(files)));
    }
    match cb.get_text() {
        Ok(t) if !t.is_empty() => Ok(Some(ClipboardContent::Text(t))),
        _ => Ok(None),
    }
}
