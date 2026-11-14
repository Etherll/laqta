//! The pixel work of a Capture job: compose the frames, encode the image once, apply the JPEG fallback,
//! and make the thumbnail and the toast preview. It runs as one compute hop, so a job makes the same pool
//! calls whatever its flags are (ADR 020, ADR 024).

use laqta_domain::workflow::ImageSettings;
use laqta_domain::{Bitmap, ImageFormat};
use laqta_imaging::{CodecError, Codecs, EncodeOptions, compose_monitors, thumbnail};
use laqta_ports::AppError;

use super::CaptureImage;

/// Longest side of the toast preview, in pixels: sharp at the widest toast (600 px) on a 2x display.
const PREVIEW_PIXELS: u32 = 1200;

/// An encoded image: its bytes and the format they are in.
#[derive(Clone, Debug)]
pub(super) struct Encoded {
    pub(super) bytes: Vec<u8>,
    pub(super) format: ImageFormat,
}

/// Which parts of the pixel work a job needs.
#[derive(Clone, Copy, Debug)]
pub(super) struct Plan {
    pub(super) encode: bool,
    pub(super) thumbnail: bool,
    pub(super) preview: bool,
}

/// What `prepare` produced. A part is `Some` only when the plan asked for it.
#[derive(Debug)]
pub(super) struct Prepared {
    /// The composed image, or the cropped image as it was given.
    pub(super) image: Bitmap,
    pub(super) encoded: Option<Encoded>,
    /// The thumbnail, in the same format as `encoded`. An encode error here is a step failure, not a task failure.
    pub(super) thumbnail: Option<Result<Encoded, AppError>>,
    /// A copy of the image, at most `PREVIEW_PIXELS` on each side, for the toast.
    pub(super) preview: Option<Bitmap>,
    /// Set when the format could not hold the image and PNG was used instead. The job reports it as a step failure.
    pub(super) warning: Option<String>,
}

/// Runs on the compute pool.
pub(super) fn prepare(codecs: &Codecs, input: CaptureImage, plan: Plan, settings: &ImageSettings) -> Result<Prepared, AppError> {
    let image = match input {
        CaptureImage::Frames(frames) => compose_monitors(&frames),
        CaptureImage::Image(image) => image,
    };
    let (encoded, warning) = if plan.encode {
        let (encoded, warning) = encode_image(codecs, &image, settings)?;
        (Some(encoded), warning)
    } else {
        (None, None)
    };
    let thumb = match &encoded {
        Some(full) if plan.thumbnail => Some(encode_thumbnail(codecs, &image, settings, full.format)),
        _ => None,
    };
    let preview = plan.preview.then(|| thumbnail(&image, PREVIEW_PIXELS, PREVIEW_PIXELS));
    Ok(Prepared { image, encoded, thumbnail: thumb, preview, warning })
}

/// Encodes in the configured format. When the result is larger than `auto_jpeg_above_bytes`, and the format
/// is not JPEG already, it is encoded again as JPEG. A format that cannot hold the image's size (JPEG and GIF
/// stop at 65535 pixels per side) falls back to PNG, and the warning says so; a tall image never loses the capture
/// to its format. The JPEG switch of a too-large PNG is simply not made.
fn encode_image(codecs: &Codecs, image: &Bitmap, settings: &ImageSettings) -> Result<(Encoded, Option<String>), AppError> {
    let (bytes, format, warning) = match encode_as(codecs, image, settings.format, settings.jpeg_quality) {
        Ok(bytes) => (bytes, settings.format, None),
        Err(CodecError::TooLarge { width, height }) if settings.format != ImageFormat::Png => {
            let bytes = encode_as(codecs, image, ImageFormat::Png, settings.jpeg_quality).map_err(io_error)?;
            (bytes, ImageFormat::Png, Some(format!("{width}x{height} is too large for {:?}; saved as PNG", settings.format)))
        }
        Err(error) => return Err(io_error(error)),
    };
    let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    let too_large = settings.auto_jpeg_above_bytes.is_some_and(|limit| size > limit);
    if too_large && format != ImageFormat::Jpeg && warning.is_none() {
        match encode_as(codecs, image, ImageFormat::Jpeg, settings.jpeg_quality) {
            Ok(bytes) => return Ok((Encoded { bytes, format: ImageFormat::Jpeg }, None)),
            Err(CodecError::TooLarge { .. }) => {}
            Err(error) => return Err(io_error(error)),
        }
    }
    Ok((Encoded { bytes, format }, warning))
}

fn encode_thumbnail(codecs: &Codecs, image: &Bitmap, settings: &ImageSettings, format: ImageFormat) -> Result<Encoded, AppError> {
    let small = thumbnail(image, settings.thumbnail_width, settings.thumbnail_height);
    let bytes = encode_as(codecs, &small, format, settings.jpeg_quality).map_err(io_error)?;
    Ok(Encoded { bytes, format })
}

fn encode_as(codecs: &Codecs, image: &Bitmap, format: ImageFormat, jpeg_quality: u8) -> Result<Vec<u8>, CodecError> {
    codecs.encode(image, &EncodeOptions { format, jpeg_quality })
}

fn io_error(error: CodecError) -> AppError {
    AppError::Io(error.to_string())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::disallowed_types)]
    use laqta_domain::PixelFormat;

    use super::*;

    fn in_compute_pool<R: Send>(body: impl FnOnce() -> R + Send) -> R {
        rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap().install(body)
    }

    fn strip(width: u32) -> Bitmap {
        Bitmap::new(width, 1, PixelFormat::Rgba8, vec![255; 4 * width as usize].into()).unwrap()
    }

    #[test]
    fn an_image_too_large_for_jpeg_or_gif_is_saved_as_png_with_a_warning() {
        in_compute_pool(|| {
            for format in [ImageFormat::Jpeg, ImageFormat::Gif] {
                let settings = ImageSettings { format, ..ImageSettings::default() };
                let codecs = Codecs::builtin();
                let (encoded, warning) = encode_image(&codecs, &strip(65_536), &settings).unwrap();
                assert_eq!(encoded.format, ImageFormat::Png, "{format:?}");
                assert!(warning.unwrap().contains("65536x1"));
            }
        });
    }

    #[test]
    fn the_jpeg_switch_is_skipped_for_an_image_jpeg_cannot_hold() {
        in_compute_pool(|| {
            let settings = ImageSettings { auto_jpeg_above_bytes: Some(1), ..ImageSettings::default() };
            let codecs = Codecs::builtin();
            let (encoded, warning) = encode_image(&codecs, &strip(65_536), &settings).unwrap();
            assert_eq!((encoded.format, warning), (ImageFormat::Png, None));
            let (small, _) = encode_image(&codecs, &strip(8), &settings).unwrap();
            assert_eq!(small.format, ImageFormat::Jpeg, "a normal image still switches");
        });
    }
}
