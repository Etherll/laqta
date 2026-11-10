//! Text recognition (`Ocr`) through Vision's `VNRecognizeTextRequest`. Vision needs no permission
//! and runs on the CPU/ANE inside the calling thread, so each call goes to the blocking pool.

use laqta_domain::geometry::PhysRect;
use laqta_domain::{Bitmap, PixelFormat};
use laqta_ports::{BoxFuture, Ocr, OcrLine, OcrResult, PortError};
use objc2::rc::autoreleasepool;
use objc2::runtime::NSObjectProtocol;
use objc2::{AnyThread, sel};
use objc2_core_foundation::{CFData, CFRetained};
use objc2_core_graphics::{
    CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage, CGImageAlphaInfo, CGImageByteOrderInfo, kCGColorSpaceSRGB,
};
use objc2_foundation::{NSArray, NSDictionary, NSString};
use objc2_vision::{VNImageRequestHandler, VNRecognizeTextRequest, VNRequest, VNRequestTextRecognitionLevel};

/// A Vision bounding box as a pixel rectangle. Vision's box is normalised (0..1 of the image) with
/// its origin at the BOTTOM-left; `PhysRect` is in pixels with its origin at the TOP-left, so the
/// y axis flips. Each edge is rounded on its own (like the Windows adapter) so neighbours do not
/// drift, and the result is kept inside the image.
fn pixel_rect(x: f64, y: f64, w: f64, h: f64, image_width: u32, image_height: u32) -> PhysRect {
    let (iw, ih) = (f64::from(image_width), f64::from(image_height));
    let edge = |value: f64, max: f64| value.round().clamp(0.0, max) as i32;
    let left = edge(x * iw, iw);
    let right = edge((x + w) * iw, iw);
    let top = edge((1.0 - (y + h)) * ih, ih);
    let bottom = edge((1.0 - y) * ih, ih);
    PhysRect { x: left, y: top, width: (right - left).max(0) as u32, height: (bottom - top).max(0) as u32 }
}

/// The tag in `supported` that `requested` names: an exact match ignoring case (and `_` for `-`),
/// else the first one with the same primary subtag, so "en" finds "en-US".
fn pick_language<'a>(requested: &str, supported: &'a [String]) -> Option<&'a str> {
    let normal = |tag: &str| tag.replace('_', "-").to_ascii_lowercase();
    let wanted = normal(requested);
    let primary = |tag: &str| normal(tag).split('-').next().unwrap_or_default().to_owned();
    supported
        .iter()
        .find(|tag| normal(tag) == wanted)
        .or_else(|| supported.iter().find(|tag| primary(tag) == primary(requested)))
        .map(String::as_str)
}

/// Whether `language` asks for automatic detection (empty or "auto").
fn is_auto(language: &str) -> bool {
    language.is_empty() || language.eq_ignore_ascii_case("auto")
}

/// CGImage flags for the pixel order. Alpha stays straight (not premultiplied), as in `Bitmap`.
fn bitmap_info(format: PixelFormat) -> CGBitmapInfo {
    let bits = match format {
        // Bytes R, G, B, A in memory: alpha last, 32-bit big-endian word order.
        PixelFormat::Rgba8 => CGImageAlphaInfo::Last.0 | CGImageByteOrderInfo::Order32Big.0,
        // Bytes B, G, R, A in memory: read as a little-endian word that is A, R, G, B, so alpha first.
        PixelFormat::Bgra8 => CGImageAlphaInfo::First.0 | CGImageByteOrderInfo::Order32Little.0,
    };
    CGBitmapInfo(bits)
}

/// The Vision adapter. It holds no state: each call builds its own request.
#[derive(Debug, Default)]
pub struct VisionOcr;

impl Ocr for VisionOcr {
    fn recognize(&self, image: Bitmap, language: String) -> BoxFuture<'_, Result<OcrResult, PortError>> {
        Box::pin(blocking::unblock(move || recognize_blocking(&image, &language)))
    }

    fn languages(&self) -> Vec<String> {
        autoreleasepool(|_| {
            let request = VNRecognizeTextRequest::new();
            request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
            supported_languages(&request).unwrap_or_else(|e| {
                tracing::warn!("listing the Vision OCR languages failed: {e}");
                Vec::new()
            })
        })
    }
}

/// The whole recognition on the calling thread: image, request, handler and line mapping.
fn recognize_blocking(image: &Bitmap, language: &str) -> Result<OcrResult, PortError> {
    if image.width() == 0 || image.height() == 0 {
        return Ok(OcrResult::default());
    }
    // The blocking pool's threads have no autorelease pool, and Vision returns autoreleased objects.
    autoreleasepool(|_| {
        let cg_image = cg_image(image)?;
        let request = VNRecognizeTextRequest::new();
        request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
        request.setUsesLanguageCorrection(true);
        set_language(&request, language)?;

        let options = NSDictionary::new();
        // SAFETY: `cg_image` is a live CGImage and `options` an empty dictionary; both outlive the call.
        let handler = unsafe { VNImageRequestHandler::initWithCGImage_options(VNImageRequestHandler::alloc(), &cg_image, &options) };
        let base: &VNRequest = &request;
        handler
            .performRequests_error(&NSArray::from_slice(&[base]))
            .map_err(|e| PortError::Failed(format!("Vision text recognition failed: {}", e.localizedDescription())))?;

        let mut lines = Vec::new();
        for observation in request.results().map(|found| found.to_vec()).unwrap_or_default() {
            let candidates = observation.topCandidates(1).to_vec();
            let Some(best) = candidates.first() else {
                continue;
            };
            // SAFETY: a plain getter on an observation Vision just returned.
            let bounds = unsafe { observation.boundingBox() };
            let rect = pixel_rect(bounds.origin.x, bounds.origin.y, bounds.size.width, bounds.size.height, image.width(), image.height());
            lines.push(OcrLine { rect, text: best.string().to_string(), confidence: best.confidence(), words: Vec::new() });
        }
        // Reading order: top to bottom, then left to right. The sort is stable.
        lines.sort_by_key(|line| (line.rect.y, line.rect.x));
        Ok(OcrResult { lines })
    })
}

/// Picks the recognition language: automatic detection for "" and "auto", else `language` mapped
/// onto a tag Vision supports. A language Vision cannot read is `Unsupported`, like on Windows.
fn set_language(request: &VNRecognizeTextRequest, language: &str) -> Result<(), PortError> {
    if is_auto(language) {
        // `automaticallyDetectsLanguage` is macOS 13+; older systems keep Vision's default (en-US).
        if request.respondsToSelector(sel!(setAutomaticallyDetectsLanguage:)) {
            request.setAutomaticallyDetectsLanguage(true);
        }
        return Ok(());
    }
    // If the list cannot be read, hand the tag to Vision as it is and let it judge.
    let tag = match supported_languages(request) {
        Ok(supported) if !supported.is_empty() => pick_language(language, &supported)
            .map(str::to_owned)
            .ok_or_else(|| PortError::Unsupported(format!("Vision cannot read the language \"{language}\"")))?,
        _ => language.to_owned(),
    };
    request.setRecognitionLanguages(&NSArray::from_retained_slice(&[NSString::from_str(&tag)]));
    Ok(())
}

/// The BCP-47 tags the request can recognise at its current level and revision.
fn supported_languages(request: &VNRecognizeTextRequest) -> Result<Vec<String>, String> {
    // Added in macOS 12; an unknown selector would raise an Objective-C exception.
    if !request.respondsToSelector(sel!(supportedRecognitionLanguagesAndReturnError:)) {
        return Err("needs macOS 12 or later".to_owned());
    }
    // SAFETY: an instance method with no arguments besides the error out-parameter the binding handles.
    let list = unsafe { request.supportedRecognitionLanguagesAndReturnError() }.map_err(|e| e.localizedDescription().to_string())?;
    Ok(list.to_vec().iter().map(|tag| tag.to_string()).collect())
}

/// The bitmap as an sRGB CGImage. The pixels are copied, so the image does not borrow `bitmap`.
fn cg_image(bitmap: &Bitmap) -> Result<CFRetained<CGImage>, PortError> {
    let failed = |what: &str| PortError::Failed(format!("could not make an image for Vision: {what}"));
    let (width, height) = (bitmap.width() as usize, bitmap.height() as usize);
    let data = CFData::from_bytes(bitmap.pixels());
    let provider = CGDataProvider::with_cf_data(Some(&data)).ok_or_else(|| failed("data provider"))?;
    // SAFETY: a constant CFString that CoreGraphics initialises when it loads.
    let srgb = unsafe { kCGColorSpaceSRGB };
    let space = CGColorSpace::with_name(Some(srgb)).ok_or_else(|| failed("colour space"))?;
    // SAFETY: the size, bits per component (8), bits per pixel (32) and row length (4 bytes per pixel,
    // tightly packed, as `Bitmap` guarantees) match the buffer; `decode` is null, which means none.
    let image = unsafe {
        CGImage::new(
            width,
            height,
            8,
            32,
            width * 4,
            Some(&space),
            bitmap_info(bitmap.format()),
            Some(&provider),
            std::ptr::null(),
            false,
            CGColorRenderingIntent::RenderingIntentDefault,
        )
    };
    image.ok_or_else(|| failed("image"))
}

#[cfg(test)]
mod tests {
    use super::{PixelFormat, pick_language, pixel_rect};
    use laqta_domain::geometry::PhysRect;

    #[test]
    fn a_box_at_the_bottom_left_lands_at_the_bottom_left_of_the_pixels() {
        // Normalised (0, 0, 0.5, 0.25) of a 200x100 image: the lower-left quarter-height strip.
        let rect = pixel_rect(0.0, 0.0, 0.5, 0.25, 200, 100);
        assert_eq!(rect, PhysRect { x: 0, y: 75, width: 100, height: 25 });
    }

    #[test]
    fn a_box_at_the_top_right_lands_at_the_top_right_of_the_pixels() {
        let rect = pixel_rect(0.5, 0.75, 0.5, 0.25, 200, 100);
        assert_eq!(rect, PhysRect { x: 100, y: 0, width: 100, height: 25 });
    }

    #[test]
    fn a_box_past_the_edge_is_clipped_to_the_image() {
        let rect = pixel_rect(0.9, 0.9, 0.5, 0.5, 100, 100);
        assert_eq!(rect, PhysRect { x: 90, y: 0, width: 10, height: 10 });
    }

    #[test]
    fn languages_match_exactly_then_by_primary_subtag() {
        let supported = ["en-US".to_owned(), "zh-Hans".to_owned(), "zh-Hant".to_owned()];
        assert_eq!(pick_language("en-us", &supported), Some("en-US"));
        assert_eq!(pick_language("zh_Hant", &supported), Some("zh-Hant"));
        assert_eq!(pick_language("en", &supported), Some("en-US"));
        assert_eq!(pick_language("zh", &supported), Some("zh-Hans"));
        assert_eq!(pick_language("fr", &supported), None);
    }

    #[test]
    fn rgba_and_bgra_set_different_byte_order_flags() {
        assert_ne!(super::bitmap_info(PixelFormat::Rgba8), super::bitmap_info(PixelFormat::Bgra8));
    }
}
