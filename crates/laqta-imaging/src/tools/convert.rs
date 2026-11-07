//! Re-encodes an image file into another format, and resizes it on the way (M5 image converter and resizer).

use crate::ops;
use crate::{CodecError, Codecs, EncodeOptions};

/// Decodes `bytes` in any supported format and encodes the picture as `to`. Runs on the compute pool, because the
/// encoders do.
pub fn convert(bytes: &[u8], codecs: &Codecs, to: EncodeOptions) -> Result<Vec<u8>, CodecError> {
    debug_assert!(rayon::current_thread_index().is_some(), "convert must run inside the compute pool");
    let img = codecs.decode(bytes)?;
    codecs.encode(&img, &to)
}

/// Decodes `bytes`, resizes the picture and encodes it as `to`. Without `keep_aspect`, the result is exactly `width` x
/// `height`, where a 0 side keeps the source's size on that axis. With `keep_aspect`, the picture is scaled to fit
/// inside `width` x `height`, where a 0 side is unbounded, so it can grow as well as shrink. A picture that already has
/// the target size is not resampled. Runs on the compute pool.
pub fn resize_bytes(
    bytes: &[u8],
    codecs: &Codecs,
    width: u32,
    height: u32,
    keep_aspect: bool,
    to: EncodeOptions,
) -> Result<Vec<u8>, CodecError> {
    debug_assert!(rayon::current_thread_index().is_some(), "resize_bytes must run inside the compute pool");
    let img = codecs.decode(bytes)?;
    let (w, h) = target_size((img.width(), img.height()), (width, height), keep_aspect);
    let out = if (w, h) == (img.width(), img.height()) { img } else { ops::resize(&img, w, h) };
    codecs.encode(&out, &to)
}

/// The output size for `resize_bytes`, from the source size, the requested box and whether the aspect is kept.
fn target_size((iw, ih): (u32, u32), (width, height): (u32, u32), keep_aspect: bool) -> (u32, u32) {
    if !keep_aspect {
        return (if width == 0 { iw } else { width }, if height == 0 { ih } else { height });
    }
    if iw == 0 || ih == 0 || (width == 0 && height == 0) {
        return (iw, ih);
    }
    let mut scale = f64::INFINITY;
    if width > 0 {
        scale = scale.min(f64::from(width) / f64::from(iw));
    }
    if height > 0 {
        scale = scale.min(f64::from(height) / f64::from(ih));
    }
    let w = (f64::from(iw) * scale).round().max(1.0) as u32;
    let h = (f64::from(ih) * scale).round().max(1.0) as u32;
    (w, h)
}
