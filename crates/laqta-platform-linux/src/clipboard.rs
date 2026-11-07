use std::borrow::Cow;
use std::sync::Mutex;

use laqta_domain::{Bitmap, PixelFormat};
use laqta_ports::{Clipboard, ClipboardContent, PortError};

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

    fn set_text(&self, text: &str) -> Result<(), PortError> {
        self.with(|cb| cb.set_text(text.to_string()).map_err(|e| PortError::Failed(e.to_string())))
    }

    fn set_files(&self, paths: &[std::path::PathBuf]) -> Result<(), PortError> {
        self.with(|cb| cb.set().file_list(paths).map_err(|e| PortError::Failed(e.to_string())))
    }

    fn get(&self) -> Result<Option<ClipboardContent>, PortError> {
        self.with(read_clipboard)
    }
}

impl ArboardClipboard {
    fn with<T>(&self, f: impl FnOnce(&mut arboard::Clipboard) -> Result<T, PortError>) -> Result<T, PortError> {
        let mut guard = self.inner.lock().map_err(|_| PortError::Failed("clipboard lock poisoned".into()))?;
        if guard.is_none() {
            *guard = Some(arboard::Clipboard::new().map_err(|e| PortError::Failed(e.to_string()))?);
        }
        let cb = guard.as_mut().ok_or_else(|| PortError::Failed("clipboard unavailable".into()))?;
        f(cb)
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
