//! Thumbnails of several images at one bound (M5 thumbnailer).

use laqta_domain::Bitmap;

use crate::ops;

/// The thumbnail of each image, in order, with `ops::thumbnail`: each image is scaled down to fit `max_w` x `max_h`
/// (a 0 bound is unbounded), and an image that already fits keeps its size. Runs on the compute pool.
pub fn thumbnails(images: &[Bitmap], max_w: u32, max_h: u32) -> Vec<Bitmap> {
    debug_assert!(rayon::current_thread_index().is_some(), "thumbnails must run inside the compute pool");
    images.iter().map(|img| ops::thumbnail(img, max_w, max_h)).collect()
}
