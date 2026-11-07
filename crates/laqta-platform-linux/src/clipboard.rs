use std::borrow::Cow;
use std::sync::Mutex;

use laqta_domain::{Bitmap, PixelFormat};
use laqta_ports::{Clipboard, PortError};

/// Keeps one `arboard::Clipboard` alive: on X11 and Wayland the owner must live for the content to
/// stay pasteable.
#[derive(Default)]
pub struct ArboardClipboard {
    inner: Mutex<Option<arboard::Clipboard>>,
}

impl Clipboard for ArboardClipboard {
    fn set_image(&self, image: &Bitmap) -> Result<(), PortError> {
        let bytes: Cow<'_, [u8]> = match image.format() {
            PixelFormat::Rgba8 => image.pixels().into(),
            PixelFormat::Bgra8 => {
                image.pixels().as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0], p[3]]).collect::<Vec<_>>().into()
            }
        };
        let mut guard = self.inner.lock().map_err(|_| PortError::Failed("clipboard lock poisoned".into()))?;
        if guard.is_none() {
            *guard = Some(arboard::Clipboard::new().map_err(|e| PortError::Failed(e.to_string()))?);
        }
        let cb = guard.as_mut().ok_or_else(|| PortError::Failed("clipboard unavailable".into()))?;
        cb.set_image(arboard::ImageData { width: image.width() as usize, height: image.height() as usize, bytes })
            .map_err(|e| PortError::Failed(e.to_string()))
    }
}
