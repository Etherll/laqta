//! Bitmaps as GPUI images.

use std::sync::Arc;

use gpui::RenderImage;
use laqta_domain::Bitmap;
use smallvec::SmallVec;

/// Copies `bitmap` into a GPUI image (BGRA). A 4K frame is a 33 MB copy, so callers keep it off
/// the GPUI thread where they can.
pub(crate) fn render_image(bitmap: &Bitmap) -> Option<Arc<RenderImage>> {
    let buffer = image::RgbaImage::from_raw(bitmap.width(), bitmap.height(), laqta_imaging::bgra8_pixels(bitmap))?;
    Some(Arc::new(RenderImage::new(SmallVec::<[image::Frame; 1]>::from_elem(image::Frame::new(buffer), 1))))
}
