//! Windows OCR: `Windows.Media.Ocr` behind the `Ocr` port (ADR 023). The pixel conversion,
//! the downscale and the rectangle maths are pure and tested on every target. The WinRT calls are
//! Windows-only, and each recognition runs on a thread of its own that holds a COM apartment.

use laqta_domain::geometry::PhysRect;
use laqta_domain::{Bitmap, PixelFormat};

#[cfg(windows)]
use laqta_ports::{BoxFuture, Ocr, OcrLine, OcrResult, OcrWord, PortError};
#[cfg(windows)]
use windows::Globalization::Language;
#[cfg(windows)]
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
#[cfg(windows)]
use windows::Media::Ocr::OcrEngine;
#[cfg(windows)]
use windows::Storage::Streams::DataWriter;
#[cfg(windows)]
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
#[cfg(windows)]
use windows::core::HSTRING;

/// The engine reads premultiplied BGRA8 rows. Laqta bitmaps are straight alpha (`laqta-domain`
/// `image.rs`), so the colour channels are scaled by alpha, and RGBA is swapped to BGRA.
fn to_premultiplied_bgra(bitmap: &Bitmap) -> Vec<u8> {
    let rgba = bitmap.format() == PixelFormat::Rgba8;
    let mut out = Vec::with_capacity(bitmap.pixels().len());
    for &[c0, c1, c2, alpha] in bitmap.pixels().as_chunks::<4>().0 {
        let (blue, green, red) = if rgba { (c2, c1, c0) } else { (c0, c1, c2) };
        out.extend_from_slice(&[premultiply(blue, alpha), premultiply(green, alpha), premultiply(red, alpha), alpha]);
    }
    out
}

/// `channel` scaled by `alpha`, rounded to the nearest 8-bit value.
fn premultiply(channel: u8, alpha: u8) -> u8 {
    ((u32::from(channel) * u32::from(alpha) + 127) / 255) as u8
}

/// The engine's input size: the bitmap as it is, or scaled down with its aspect ratio to fit
/// `max_dimension` (the engine's `MaxImageDimension`). A zero limit leaves the size alone.
fn fit_within(width: u32, height: u32, max_dimension: u32) -> (u32, u32) {
    if max_dimension == 0 || width.max(height) <= max_dimension {
        return (width, height);
    }
    let scale = f64::from(max_dimension) / f64::from(width.max(height));
    let fit = |side: u32| ((f64::from(side) * scale).round() as u32).clamp(1, max_dimension);
    (fit(width), fit(height))
}

/// Area-averaged resize of BGRA8 rows to `to_width` by `to_height`. Each output pixel is the mean of
/// the source pixels under it, so thin strokes keep their grey. The target is never larger.
fn downscale_bgra(pixels: &[u8], width: u32, height: u32, to_width: u32, to_height: u32) -> Vec<u8> {
    let (src_w, src_h) = (width as usize, height as usize);
    let (dst_w, dst_h) = (to_width as usize, to_height as usize);
    let mut out = vec![0u8; dst_w * dst_h * 4];
    for dy in 0..dst_h {
        let (y0, y1) = span(dy, dst_h, src_h);
        for dx in 0..dst_w {
            let (x0, x1) = span(dx, dst_w, src_w);
            let mut sum = [0u32; 4];
            for row in y0..y1 {
                for px in pixels[(row * src_w + x0) * 4..(row * src_w + x1) * 4].as_chunks::<4>().0 {
                    for (total, &channel) in sum.iter_mut().zip(px) {
                        *total += u32::from(channel);
                    }
                }
            }
            let count = ((y1 - y0) * (x1 - x0)) as u32;
            let at = (dy * dst_w + dx) * 4;
            for (out_channel, total) in out[at..at + 4].iter_mut().zip(sum) {
                *out_channel = ((total + count / 2) / count) as u8;
            }
        }
    }
    out
}

/// The source range `[start, end)` that output pixel `index` of `out_len` covers. Never empty.
fn span(index: usize, out_len: usize, in_len: usize) -> (usize, usize) {
    let start = index * in_len / out_len;
    let end = ((index + 1) * in_len / out_len).clamp(start + 1, in_len);
    (start, end)
}

/// A rectangle by its edges, in the pixels of the image the engine read.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Edges {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

impl Edges {
    /// From the `Rect` (x, y, width, height) that `OcrWord::BoundingRect` returns.
    fn from_xywh(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self { left: x, top: y, right: x + width, bottom: y + height }
    }
}

/// The smallest rectangle around `edges`, or `None` when there are none (a line without words).
fn union(edges: &[Edges]) -> Option<Edges> {
    edges.iter().copied().reduce(|a, b| Edges {
        left: a.left.min(b.left),
        top: a.top.min(b.top),
        right: a.right.max(b.right),
        bottom: a.bottom.max(b.bottom),
    })
}

/// `edges` mapped back to the bitmap's physical pixels by dividing out the per-axis downscale
/// (fitted size over bitmap size). Each edge is rounded on its own, so neighbours do not drift.
fn to_physical(edges: Edges, scale_x: f32, scale_y: f32) -> PhysRect {
    let left = (edges.left / scale_x).round() as i32;
    let top = (edges.top / scale_y).round() as i32;
    let right = (edges.right / scale_x).round() as i32;
    let bottom = (edges.bottom / scale_y).round() as i32;
    PhysRect { x: left, y: top, width: (right - left).max(0) as u32, height: (bottom - top).max(0) as u32 }
}

/// The Windows OCR adapter. It holds no state: each call builds its own engine.
#[cfg(windows)]
#[derive(Debug)]
pub struct WindowsOcr;

#[cfg(windows)]
impl Ocr for WindowsOcr {
    fn recognize(&self, image: Bitmap, language: String) -> BoxFuture<'_, Result<OcrResult, PortError>> {
        Box::pin(async move {
            // The WinRT calls need their own apartment, and the blocking pool's threads are shared,
            // so each recognition gets a thread of its own. The thread starts only when polled.
            let spawned = std::thread::Builder::new().name("laqta-ocr".to_owned()).spawn(move || recognize_on_thread(image, language));
            match spawned {
                Ok(worker) => blocking::unblock(move || worker.join())
                    .await
                    .unwrap_or_else(|_| Err(PortError::Failed("the OCR thread panicked".into()))),
                Err(e) => Err(PortError::Failed(format!("could not start the OCR thread: {e}"))),
            }
        })
    }

    fn languages(&self) -> Vec<String> {
        let _apartment = ComApartment::enter();
        available_languages().unwrap_or_else(|e| {
            tracing::warn!("listing the Windows OCR languages failed: {e}");
            Vec::new()
        })
    }
}

/// The whole recognition on the calling thread: the engine, the pixels, the WinRT call and the
/// line mapping.
#[cfg(windows)]
fn recognize_on_thread(image: Bitmap, language: String) -> Result<OcrResult, PortError> {
    let _apartment = ComApartment::enter();
    let engine = create_engine(&language)?;
    let max_dimension = OcrEngine::MaxImageDimension().map_err(failed("reading the OCR size limit"))?;
    let (width, height) = (image.width(), image.height());
    let (fit_w, fit_h) = fit_within(width, height, max_dimension);
    let mut pixels = to_premultiplied_bgra(&image);
    if (fit_w, fit_h) != (width, height) {
        pixels = downscale_bgra(&pixels, width, height, fit_w, fit_h);
    }
    let bitmap = software_bitmap(&pixels, fit_w, fit_h)?;
    let result = engine.RecognizeAsync(&bitmap).map_err(failed("starting OCR"))?.join().map_err(failed("running OCR"))?;
    let (scale_x, scale_y) = (fit_w as f32 / width as f32, fit_h as f32 / height as f32);
    let lines = result.Lines().map_err(failed("reading the OCR lines"))?;
    let mut out = Vec::new();
    for index in 0..lines.Size().map_err(failed("reading the OCR lines"))? {
        let line = lines.GetAt(index).map_err(failed("reading an OCR line"))?;
        out.extend(map_line(&line, scale_x, scale_y)?);
    }
    Ok(OcrResult { lines: out })
}

/// One WinRT line as a port line: the union of its words' rectangles, or nothing when it has none.
/// The words keep their own rectangles, so the overlay can mark each one (ADR 031).
#[cfg(windows)]
fn map_line(line: &windows::Media::Ocr::OcrLine, scale_x: f32, scale_y: f32) -> Result<Option<OcrLine>, PortError> {
    let found = line.Words().map_err(failed("reading OCR words"))?;
    let mut edges = Vec::new();
    let mut words = Vec::new();
    for index in 0..found.Size().map_err(failed("reading OCR words"))? {
        let word = found.GetAt(index).map_err(failed("reading an OCR word"))?;
        let rect = word.BoundingRect().map_err(failed("reading an OCR word's rectangle"))?;
        let edge = Edges::from_xywh(rect.X, rect.Y, rect.Width, rect.Height);
        let text = word.Text().map_err(failed("reading an OCR word"))?.to_string();
        words.push(OcrWord { rect: to_physical(edge, scale_x, scale_y), text });
        edges.push(edge);
    }
    let Some(bounds) = union(&edges) else {
        return Ok(None);
    };
    let text = line.Text().map_err(failed("reading an OCR line"))?.to_string();
    Ok(Some(OcrLine { rect: to_physical(bounds, scale_x, scale_y), text, confidence: 1.0, words }))
}

/// The engine for `language` (a BCP-47 tag), or for the user's profile languages when it is empty.
#[cfg(windows)]
fn create_engine(language: &str) -> Result<OcrEngine, PortError> {
    let engine = if language.is_empty() {
        OcrEngine::TryCreateFromUserProfileLanguages()
    } else {
        let tag = HSTRING::from(language);
        Language::CreateLanguage(&tag).and_then(|lang| OcrEngine::TryCreateFromLanguage(&lang))
    };
    // WinRT hands back a null engine when no recognizer is installed; windows-rs reports that as an error.
    engine.map_err(|e| PortError::Unsupported(format!("no Windows OCR engine for this language ({e})")))
}

/// A `SoftwareBitmap` of premultiplied BGRA8 pixels, copied in through a `DataWriter` buffer.
#[cfg(windows)]
fn software_bitmap(pixels: &[u8], width: u32, height: u32) -> Result<SoftwareBitmap, PortError> {
    let writer = DataWriter::new().map_err(failed("starting the OCR bitmap"))?;
    writer.WriteBytes(pixels).map_err(failed("copying the OCR bitmap"))?;
    let buffer = writer.DetachBuffer().map_err(failed("copying the OCR bitmap"))?;
    SoftwareBitmap::CreateCopyWithAlphaFromBuffer(
        &buffer,
        BitmapPixelFormat::Bgra8,
        width as i32,
        height as i32,
        BitmapAlphaMode::Premultiplied,
    )
    .map_err(failed("creating the OCR bitmap"))
}

/// The BCP-47 tags of the recognizers installed on this machine.
#[cfg(windows)]
fn available_languages() -> windows::core::Result<Vec<String>> {
    let languages = OcrEngine::AvailableRecognizerLanguages()?;
    let mut tags = Vec::new();
    for index in 0..languages.Size()? {
        tags.push(languages.GetAt(index)?.LanguageTag()?.to_string());
    }
    Ok(tags)
}

/// Maps a WinRT error to a port failure, with the step that failed.
#[cfg(windows)]
fn failed(context: &'static str) -> impl FnOnce(windows::core::Error) -> PortError {
    move |e| PortError::Failed(format!("{context}: {e}"))
}

/// Initialises COM on this thread and uninitialises it on drop when this call started it. `None`
/// when COM is already set up in another mode (`RPC_E_CHANGED_MODE`), which WinRT then uses as is.
#[cfg(windows)]
struct ComApartment;

#[cfg(windows)]
impl ComApartment {
    fn enter() -> Option<Self> {
        // SAFETY: plain initialisation with no reserved pointer.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        // S_OK and S_FALSE both take a reference that `Drop` releases.
        hr.is_ok().then_some(ComApartment)
    }
}

#[cfg(windows)]
impl Drop for ComApartment {
    fn drop(&mut self) {
        // SAFETY: balances the successful CoInitializeEx in `enter`.
        unsafe { CoUninitialize() };
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn bitmap(width: u32, height: u32, format: PixelFormat, pixels: Vec<u8>) -> Bitmap {
        Bitmap::new(width, height, format, Arc::from(pixels)).expect("the pixel count matches the size")
    }

    #[test]
    fn premultiply_scales_colour_by_alpha_and_keeps_opaque_pixels() {
        let pixels = vec![200, 100, 50, 128, 9, 9, 9, 0, 1, 2, 3, 255];
        let out = to_premultiplied_bgra(&bitmap(3, 1, PixelFormat::Bgra8, pixels));
        assert_eq!(out, vec![100, 50, 25, 128, 0, 0, 0, 0, 1, 2, 3, 255]);
    }

    #[test]
    fn rgba_input_is_swapped_to_bgra() {
        let out = to_premultiplied_bgra(&bitmap(1, 1, PixelFormat::Rgba8, vec![255, 0, 0, 255]));
        assert_eq!(out, vec![0, 0, 255, 255]);
    }

    #[test]
    fn fit_keeps_small_images_and_scales_large_ones_by_aspect_ratio() {
        assert_eq!(fit_within(1920, 1080, 2600), (1920, 1080));
        assert_eq!(fit_within(2600, 100, 2600), (2600, 100));
        assert_eq!(fit_within(4000, 2000, 2000), (2000, 1000));
        assert_eq!(fit_within(1000, 4000, 2000), (500, 2000));
        assert_eq!(fit_within(10_000, 1, 100), (100, 1));
        assert_eq!(fit_within(4000, 2000, 0), (4000, 2000));
    }

    #[test]
    fn downscale_averages_each_block() {
        // 2x2 to 1x1: the channel values 0, 100, 200 and 255 average to 139 with rounding.
        let square = vec![0, 0, 0, 0, 100, 100, 100, 100, 200, 200, 200, 200, 255, 255, 255, 255];
        assert_eq!(downscale_bgra(&square, 2, 2, 1, 1), vec![139, 139, 139, 139]);
        // 4x1 to 2x1: the pairs 10, 20 and 30, 40 average to 15 and 35.
        let row = vec![10, 10, 10, 10, 20, 20, 20, 20, 30, 30, 30, 30, 40, 40, 40, 40];
        assert_eq!(downscale_bgra(&row, 4, 1, 2, 1), vec![15, 15, 15, 15, 35, 35, 35, 35]);
    }

    #[test]
    fn union_covers_every_word_and_maps_back_to_physical_pixels() {
        let words = [Edges::from_xywh(10.0, 20.0, 30.0, 10.0), Edges::from_xywh(50.0, 22.0, 20.0, 14.0)];
        let bounds = union(&words).expect("two words");
        assert_eq!(to_physical(bounds, 1.0, 1.0), PhysRect { x: 10, y: 20, width: 60, height: 16 });
        // Downscaled by half on both axes, so the rectangle doubles back to physical pixels.
        assert_eq!(to_physical(bounds, 0.5, 0.5), PhysRect { x: 20, y: 40, width: 120, height: 32 });
        assert_eq!(union(&[]), None);
    }

    #[test]
    fn edges_round_on_each_side() {
        let rect = Edges::from_xywh(10.4, 20.6, 4.2, 4.2);
        assert_eq!(to_physical(rect, 1.0, 1.0), PhysRect { x: 10, y: 21, width: 5, height: 4 });
    }
}
