//! The pixel work of a Capture job: compose the frames, encode the image once, apply the JPEG fallback,
//! and make the thumbnail and the toast preview. It runs as one compute hop, so a job makes the same pool
//! calls whatever its flags are (ADR 020, ADR 024).

use laqta_domain::workflow::ImageSettings;
use laqta_domain::{Bitmap, ImageFormat};
use laqta_imaging::{Codecs, EncodeOptions, compose_monitors, thumbnail};
use laqta_ports::AppError;

use super::CaptureImage;

/// Longest side of the toast preview, in pixels.
const PREVIEW_PIXELS: u32 = 256;

/// An encoded image: its bytes and the format they are in.
#[derive(Debug)]
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
}

/// Runs on the compute pool.
pub(super) fn prepare(codecs: &Codecs, input: CaptureImage, plan: Plan, settings: &ImageSettings) -> Result<Prepared, AppError> {
    let image = match input {
        CaptureImage::Frames(frames) => compose_monitors(&frames),
        CaptureImage::Image(image) => image,
    };
    let encoded = if plan.encode { Some(encode_image(codecs, &image, settings)?) } else { None };
    let thumb = match &encoded {
        Some(full) if plan.thumbnail => Some(encode_thumbnail(codecs, &image, settings, full.format)),
        _ => None,
    };
    let preview = plan.preview.then(|| thumbnail(&image, PREVIEW_PIXELS, PREVIEW_PIXELS));
    Ok(Prepared { image, encoded, thumbnail: thumb, preview })
}

/// Encodes in the configured format. When the result is larger than `auto_jpeg_above_bytes`, and the format
/// is not JPEG already, it is encoded again as JPEG.
fn encode_image(codecs: &Codecs, image: &Bitmap, settings: &ImageSettings) -> Result<Encoded, AppError> {
    let bytes = encode_as(codecs, image, settings.format, settings.jpeg_quality)?;
    let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    let too_large = settings.auto_jpeg_above_bytes.is_some_and(|limit| size > limit);
    if too_large && settings.format != ImageFormat::Jpeg {
        let bytes = encode_as(codecs, image, ImageFormat::Jpeg, settings.jpeg_quality)?;
        return Ok(Encoded { bytes, format: ImageFormat::Jpeg });
    }
    Ok(Encoded { bytes, format: settings.format })
}

fn encode_thumbnail(codecs: &Codecs, image: &Bitmap, settings: &ImageSettings, format: ImageFormat) -> Result<Encoded, AppError> {
    let small = thumbnail(image, settings.thumbnail_width, settings.thumbnail_height);
    let bytes = encode_as(codecs, &small, format, settings.jpeg_quality)?;
    Ok(Encoded { bytes, format })
}

fn encode_as(codecs: &Codecs, image: &Bitmap, format: ImageFormat, jpeg_quality: u8) -> Result<Vec<u8>, AppError> {
    codecs.encode(image, &EncodeOptions { format, jpeg_quality }).map_err(|error| AppError::Io(error.to_string()))
}
