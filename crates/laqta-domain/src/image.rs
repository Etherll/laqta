//! Bitmaps and captured frames. Straight (not premultiplied) alpha, tightly packed rows.

use std::sync::Arc;

use crate::geometry::Monitor;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    Bgra8,
    Rgba8,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BitmapError {
    #[error("pixel buffer is {actual} bytes, expected {expected} for {width}x{height}")]
    WrongLength { width: u32, height: u32, expected: usize, actual: usize },
}

/// An image whose pixels are shared through an `Arc`, so clones are cheap.
#[derive(Clone, Debug)]
pub struct Bitmap {
    width: u32,
    height: u32,
    format: PixelFormat,
    pixels: Arc<[u8]>,
}

impl Bitmap {
    /// Checks that `pixels.len() == width * height * 4`.
    pub fn new(width: u32, height: u32, format: PixelFormat, pixels: Arc<[u8]>) -> Result<Self, BitmapError> {
        let expected = width as usize * height as usize * 4;
        if pixels.len() != expected {
            return Err(BitmapError::WrongLength { width, height, expected, actual: pixels.len() });
        }
        Ok(Bitmap { width, height, format, pixels })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn format(&self) -> PixelFormat {
        self.format
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// The shared pixel buffer.
    pub fn shared_pixels(&self) -> Arc<[u8]> {
        self.pixels.clone()
    }
}

/// One monitor's captured image.
#[derive(Clone, Debug)]
pub struct Frame {
    pub monitor: Monitor,
    pub image: Bitmap,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_checks_length() {
        assert!(Bitmap::new(2, 2, PixelFormat::Rgba8, vec![0; 16].into()).is_ok());
        let err = Bitmap::new(2, 2, PixelFormat::Rgba8, vec![0; 15].into());
        assert!(matches!(err, Err(BitmapError::WrongLength { expected: 16, actual: 15, .. })));
    }
}
