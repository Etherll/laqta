//! Image codecs for PNG, JPEG, GIF, BMP, TIFF and lossless WebP, plus the extension hook that lets
//! `main` add codecs (ARCHITECTURE §2.1). Encoders take RGBA8 or BGRA8. Decoders return RGBA8 with
//! straight alpha. Extensions are tried in the order they were added, before the built-ins.

use std::borrow::Cow;
use std::io::Cursor;
use std::sync::Arc;

use laqta_domain::{Bitmap, ImageFormat, PixelFormat};

use crate::bmp;
use crate::png;

#[derive(Debug, Clone, thiserror::Error)]
pub enum CodecError {
    #[error("unsupported format {0:?}")]
    Unsupported(ImageFormat),
    #[error("unknown image format")]
    Unknown,
    #[error("encode: {0}")]
    Encode(String),
    #[error("decode: {0}")]
    Decode(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EncodeOptions {
    pub format: ImageFormat,
    /// Used by JPEG only. Values outside 1..=100 are clamped.
    pub jpeg_quality: u8,
}

/// A codec added to `Codecs` by the caller. It takes precedence over the built-in codec for any
/// format it claims.
pub trait CodecExtension: Send + Sync + std::fmt::Debug {
    fn name(&self) -> &str;
    fn can_encode(&self, f: ImageFormat) -> bool;
    fn can_decode(&self, f: ImageFormat) -> bool;
    fn encode(&self, img: &Bitmap, o: &EncodeOptions) -> Result<Vec<u8>, CodecError>;
    fn decode(&self, f: ImageFormat, bytes: &[u8]) -> Result<Bitmap, CodecError>;
}

/// The codecs available to encoders and decoders. `main` builds it and passes it in; there is no
/// global registry.
#[derive(Clone, Debug)]
pub struct Codecs {
    extensions: Vec<Arc<dyn CodecExtension>>,
}

impl Codecs {
    /// The built-in codecs for all six formats, with no extensions.
    pub fn builtin() -> Codecs {
        Codecs { extensions: Vec::new() }
    }

    /// Adds an extension. Extensions are tried in the order they were added, before the built-ins.
    pub fn with_extension(mut self, ext: Arc<dyn CodecExtension>) -> Codecs {
        self.extensions.push(ext);
        self
    }

    pub fn can_encode(&self, f: ImageFormat) -> bool {
        self.extensions.iter().any(|e| e.can_encode(f)) || builtin_supports(f)
    }

    pub fn can_decode(&self, f: ImageFormat) -> bool {
        self.extensions.iter().any(|e| e.can_decode(f)) || builtin_supports(f)
    }

    /// Encodes with the first extension that claims the format, else the built-in codec.
    pub fn encode(&self, img: &Bitmap, o: &EncodeOptions) -> Result<Vec<u8>, CodecError> {
        debug_assert!(rayon::current_thread_index().is_some(), "Codecs::encode must run inside the compute pool");
        match self.extensions.iter().find(|e| e.can_encode(o.format)) {
            Some(ext) => ext.encode(img, o),
            None => encode_builtin(img, o),
        }
    }

    /// Decodes bytes in any supported format. The format is found from the magic numbers.
    pub fn decode(&self, bytes: &[u8]) -> Result<Bitmap, CodecError> {
        let format = sniff(bytes).ok_or(CodecError::Unknown)?;
        match self.extensions.iter().find(|e| e.can_decode(format)) {
            Some(ext) => ext.decode(format, bytes),
            None => decode_builtin(format, bytes),
        }
    }
}

/// The format named by the leading magic bytes, or `None` if it is not one of the six.
pub fn sniff(bytes: &[u8]) -> Option<ImageFormat> {
    const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    if bytes.starts_with(PNG_SIGNATURE) {
        Some(ImageFormat::Png)
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(ImageFormat::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(ImageFormat::Gif)
    } else if bytes.starts_with(b"BM") {
        Some(ImageFormat::Bmp)
    } else if [&b"II*\0"[..], b"MM\0*", b"II+\0", b"MM\0+"].iter().any(|magic| bytes.starts_with(magic)) {
        Some(ImageFormat::Tiff)
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some(ImageFormat::WebP)
    } else {
        None
    }
}

/// The built-in codecs cover all six formats for both directions.
fn builtin_supports(f: ImageFormat) -> bool {
    matches!(f, ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::Bmp | ImageFormat::Tiff | ImageFormat::WebP)
}

fn encode_builtin(img: &Bitmap, o: &EncodeOptions) -> Result<Vec<u8>, CodecError> {
    match o.format {
        ImageFormat::Png => png::encode_with(img, ::png::Compression::Balanced).map_err(enc_err),
        ImageFormat::Jpeg => encode_jpeg(img, o.jpeg_quality),
        ImageFormat::Gif => encode_gif(img),
        ImageFormat::Bmp => bmp::encode(img),
        ImageFormat::Tiff => encode_tiff(img),
        ImageFormat::WebP => encode_webp(img),
        f => Err(CodecError::Unsupported(f)),
    }
}

fn decode_builtin(f: ImageFormat, bytes: &[u8]) -> Result<Bitmap, CodecError> {
    match f {
        ImageFormat::Png => decode_png(bytes),
        ImageFormat::Jpeg => decode_jpeg(bytes),
        ImageFormat::Gif => decode_gif(bytes),
        ImageFormat::Bmp => bmp::decode(bytes),
        ImageFormat::Tiff => decode_tiff(bytes),
        ImageFormat::WebP => decode_webp(bytes),
        f => Err(CodecError::Unsupported(f)),
    }
}

/// GIF palette speed: 10 is the crate's suggested balance of speed and quality.
const GIF_SPEED: i32 = 10;

fn encode_jpeg(img: &Bitmap, quality: u8) -> Result<Vec<u8>, CodecError> {
    let (w, h) = dims16(img)?;
    let color = match img.format() {
        PixelFormat::Rgba8 => jpeg_encoder::ColorType::Rgba,
        PixelFormat::Bgra8 => jpeg_encoder::ColorType::Bgra,
    };
    let mut out = Vec::new();
    jpeg_encoder::Encoder::new(&mut out, quality.clamp(1, 100)).encode(img.pixels(), w, h, color).map_err(enc_err)?;
    Ok(out)
}

fn decode_jpeg(bytes: &[u8]) -> Result<Bitmap, CodecError> {
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;

    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(zune_jpeg::zune_core::bytestream::ZCursor::new(bytes), options);
    let pixels = decoder.decode().map_err(dec_err)?;
    let info = decoder.info().ok_or_else(|| CodecError::Decode("jpeg headers missing".into()))?;
    bitmap(u32::from(info.width), u32::from(info.height), pixels)
}

/// Single frame: the first frame is composited onto a transparent logical screen.
fn encode_gif(img: &Bitmap) -> Result<Vec<u8>, CodecError> {
    let (w, h) = dims16(img)?;
    let mut rgba = rgba_bytes(img).into_owned();
    let frame = ::gif::Frame::from_rgba_speed(w, h, &mut rgba, GIF_SPEED);
    let mut out = Vec::new();
    {
        let mut encoder = ::gif::Encoder::new(&mut out, w, h, &[]).map_err(enc_err)?;
        encoder.write_frame(&frame).map_err(enc_err)?;
    }
    Ok(out)
}

fn decode_gif(bytes: &[u8]) -> Result<Bitmap, CodecError> {
    let mut options = ::gif::DecodeOptions::new();
    options.set_color_output(::gif::ColorOutput::RGBA);
    let mut decoder = options.read_info(bytes).map_err(dec_err)?;
    let (screen_w, screen_h) = (usize::from(decoder.width()), usize::from(decoder.height()));
    if screen_w == 0 || screen_h == 0 {
        return Err(CodecError::Decode("gif logical screen is empty".into()));
    }
    let frame = decoder.read_next_frame().map_err(dec_err)?.ok_or_else(|| CodecError::Decode("gif has no frames".into()))?;
    let (left, top) = (usize::from(frame.left), usize::from(frame.top));
    let (frame_w, frame_h) = (usize::from(frame.width), usize::from(frame.height));
    if frame.buffer.len() != frame_w * frame_h * 4 {
        return Err(CodecError::Decode("gif frame is not RGBA".into()));
    }

    let mut canvas = vec![0u8; screen_w * screen_h * 4];
    if left < screen_w && top < screen_h {
        let cols = frame_w.min(screen_w - left);
        for y in 0..frame_h.min(screen_h - top) {
            let src = &frame.buffer[y * frame_w * 4..y * frame_w * 4 + cols * 4];
            let start = ((top + y) * screen_w + left) * 4;
            canvas[start..start + cols * 4].copy_from_slice(src);
        }
    }
    bitmap(screen_w as u32, screen_h as u32, canvas)
}

fn encode_tiff(img: &Bitmap) -> Result<Vec<u8>, CodecError> {
    use tiff::encoder::{Compression, DeflateLevel, TiffEncoder, colortype::RGBA8};

    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return Err(CodecError::Encode("empty image".into()));
    }
    let rgba = rgba_bytes(img);
    let mut buf = Cursor::new(Vec::new());
    {
        let mut encoder = TiffEncoder::new(&mut buf).map_err(enc_err)?.with_compression(Compression::Deflate(DeflateLevel::Balanced));
        encoder.write_image::<RGBA8>(w, h, &rgba).map_err(enc_err)?;
    }
    Ok(buf.into_inner())
}

fn decode_tiff(bytes: &[u8]) -> Result<Bitmap, CodecError> {
    let mut decoder = tiff::decoder::Decoder::new(Cursor::new(bytes)).map_err(dec_err)?;
    let (w, h) = decoder.dimensions().map_err(dec_err)?;
    let color = decoder.colortype().map_err(dec_err)?;
    let samples = match decoder.read_image().map_err(dec_err)? {
        tiff::decoder::DecodingResult::U8(samples) => samples,
        _ => return Err(CodecError::Decode("tiff samples are not 8-bit".into())),
    };
    let rgba = match color {
        tiff::ColorType::RGBA(8) => samples,
        tiff::ColorType::RGB(8) => samples.as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        tiff::ColorType::Gray(8) => samples.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        other => return Err(CodecError::Decode(format!("unsupported tiff colour type {other:?}"))),
    };
    bitmap(w, h, rgba)
}

fn encode_webp(img: &Bitmap) -> Result<Vec<u8>, CodecError> {
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return Err(CodecError::Encode("empty image".into()));
    }
    let rgba = rgba_bytes(img);
    let mut out = Vec::new();
    ::image_webp::WebPEncoder::new(&mut out).encode(&rgba, w, h, ::image_webp::ColorType::Rgba8).map_err(enc_err)?;
    Ok(out)
}

fn decode_webp(bytes: &[u8]) -> Result<Bitmap, CodecError> {
    let mut decoder = ::image_webp::WebPDecoder::new(Cursor::new(bytes)).map_err(dec_err)?;
    let (w, h) = decoder.dimensions();
    let has_alpha = decoder.has_alpha();
    let size = decoder.output_buffer_size().ok_or_else(|| CodecError::Decode("webp size overflows".into()))?;
    let mut buf = vec![0u8; size];
    decoder.read_image(&mut buf).map_err(dec_err)?;
    let rgba = if has_alpha { buf } else { buf.as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect() };
    bitmap(w, h, rgba)
}

fn decode_png(bytes: &[u8]) -> Result<Bitmap, CodecError> {
    let mut decoder = ::png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(::png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(dec_err)?;
    let size = reader.output_buffer_size().ok_or_else(|| CodecError::Decode("png size overflows".into()))?;
    let mut buf = vec![0u8; size];
    let info = reader.next_frame(&mut buf).map_err(dec_err)?;
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        ::png::ColorType::Rgba => buf,
        ::png::ColorType::Rgb => buf.as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        ::png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        ::png::ColorType::GrayscaleAlpha => buf.as_chunks::<2>().0.iter().flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        ::png::ColorType::Indexed => return Err(CodecError::Decode("png palette was not expanded".into())),
    };
    bitmap(info.width, info.height, rgba)
}

/// RGBA bytes for the encoders that take RGBA. BGRA input is swapped; RGBA input is borrowed.
fn rgba_bytes(img: &Bitmap) -> Cow<'_, [u8]> {
    match img.format() {
        PixelFormat::Rgba8 => Cow::Borrowed(img.pixels()),
        PixelFormat::Bgra8 => Cow::Owned(img.pixels().as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0], p[3]]).collect()),
    }
}

/// Image dimensions for formats that store them as `u16`. Rejects empty images.
fn dims16(img: &Bitmap) -> Result<(u16, u16), CodecError> {
    if img.width() == 0 || img.height() == 0 {
        return Err(CodecError::Encode("empty image".into()));
    }
    let too_big = || CodecError::Encode(format!("{}x{} exceeds 65535 pixels per side", img.width(), img.height()));
    let w = u16::try_from(img.width()).map_err(|_| too_big())?;
    let h = u16::try_from(img.height()).map_err(|_| too_big())?;
    Ok((w, h))
}

fn bitmap(width: u32, height: u32, rgba: Vec<u8>) -> Result<Bitmap, CodecError> {
    Bitmap::new(width, height, PixelFormat::Rgba8, rgba.into()).map_err(|e| CodecError::Decode(e.to_string()))
}

fn enc_err(e: impl std::fmt::Display) -> CodecError {
    CodecError::Encode(e.to_string())
}

fn dec_err(e: impl std::fmt::Display) -> CodecError {
    CodecError::Decode(e.to_string())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    const W: u32 = 12;
    const H: u32 = 10;

    fn pool() -> rayon::ThreadPool {
        #[allow(clippy::disallowed_types)]
        rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap()
    }

    fn opts(format: ImageFormat) -> EncodeOptions {
        EncodeOptions { format, jpeg_quality: 90 }
    }

    /// Opaque, 120 distinct colours (within GIF's palette), smooth enough for JPEG.
    fn gradient() -> Bitmap {
        let mut px = Vec::new();
        for y in 0..H {
            for x in 0..W {
                px.extend_from_slice(&[(x * 16) as u8, (y * 24) as u8, ((x + y) * 8) as u8, 255]);
            }
        }
        Bitmap::new(W, H, PixelFormat::Rgba8, px.into()).unwrap()
    }

    /// Alpha 0, 128 and 255 in a diagonal pattern.
    fn with_alpha() -> Bitmap {
        let mut px = Vec::new();
        for y in 0..H {
            for x in 0..W {
                let a = [0u8, 128, 255][((x + y) % 3) as usize];
                px.extend_from_slice(&[(x * 16) as u8, (y * 24) as u8, 77, a]);
            }
        }
        Bitmap::new(W, H, PixelFormat::Rgba8, px.into()).unwrap()
    }

    fn to_bgra(img: &Bitmap) -> Bitmap {
        let px: Vec<u8> = img.pixels().as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
        Bitmap::new(img.width(), img.height(), PixelFormat::Bgra8, px.into()).unwrap()
    }

    fn one_pixel(px: [u8; 4]) -> Bitmap {
        Bitmap::new(1, 1, PixelFormat::Rgba8, Vec::from(px).into()).unwrap()
    }

    fn encode(img: &Bitmap, format: ImageFormat) -> Vec<u8> {
        Codecs::builtin().encode(img, &opts(format)).unwrap()
    }

    #[test]
    fn lossless_formats_round_trip_exactly() {
        let img = gradient();
        pool().install(|| {
            for format in [ImageFormat::Png, ImageFormat::Bmp, ImageFormat::Tiff, ImageFormat::WebP] {
                let out = Codecs::builtin().decode(&encode(&img, format)).unwrap();
                assert_eq!((out.width(), out.height()), (W, H), "{format:?}");
                assert_eq!(out.format(), PixelFormat::Rgba8);
                assert_eq!(out.pixels(), img.pixels(), "{format:?}");
            }
        });
    }

    #[test]
    fn alpha_survives_lossless_round_trip() {
        let img = with_alpha();
        pool().install(|| {
            for format in [ImageFormat::Png, ImageFormat::Bmp, ImageFormat::Tiff, ImageFormat::WebP] {
                let out = Codecs::builtin().decode(&encode(&img, format)).unwrap();
                assert_eq!(out.pixels(), img.pixels(), "{format:?}");
            }
        });
    }

    #[test]
    fn gif_is_palette_exact_for_few_colours() {
        let img = gradient();
        pool().install(|| {
            let out = Codecs::builtin().decode(&encode(&img, ImageFormat::Gif)).unwrap();
            assert_eq!((out.width(), out.height()), (W, H));
            assert_eq!(out.pixels(), img.pixels());
        });
    }

    #[test]
    fn jpeg_round_trip_is_close_and_opaque() {
        let img = gradient();
        pool().install(|| {
            for quality in [0, 90, 255] {
                let codecs = Codecs::builtin();
                let bytes = codecs.encode(&img, &EncodeOptions { format: ImageFormat::Jpeg, jpeg_quality: quality }).unwrap();
                let out = codecs.decode(&bytes).unwrap();
                assert_eq!((out.width(), out.height()), (W, H));
                assert!(out.pixels().as_chunks::<4>().0.iter().all(|p| p[3] == 255), "jpeg output is opaque");
                if quality == 90 {
                    let worst = out
                        .pixels()
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .zip(img.pixels().as_chunks::<4>().0.iter())
                        .flat_map(|(a, b)| (0..3).map(move |c| a[c].abs_diff(b[c])))
                        .max()
                        .unwrap();
                    assert!(worst <= 12, "quality 90: worst channel error {worst}");
                }
            }
        });
    }

    #[test]
    fn bgra_input_encodes_to_the_same_bytes_as_rgba() {
        let rgba = with_alpha();
        let bgra = to_bgra(&rgba);
        pool().install(|| {
            for format in ImageFormat::ALL {
                assert_eq!(encode(&rgba, format), encode(&bgra, format), "{format:?}");
            }
        });
    }

    #[test]
    fn sniff_identifies_each_encoded_format() {
        let img = gradient();
        pool().install(|| {
            for format in ImageFormat::ALL {
                assert_eq!(sniff(&encode(&img, format)), Some(format), "{format:?}");
            }
        });
        assert_eq!(sniff(b"hello world"), None);
        assert_eq!(sniff(b"RIFF\0\0\0\0WAVE"), None);
        assert_eq!(sniff(&[]), None);
    }

    #[derive(Debug)]
    struct FakePng;

    impl CodecExtension for FakePng {
        fn name(&self) -> &str {
            "fake-png"
        }
        fn can_encode(&self, f: ImageFormat) -> bool {
            f == ImageFormat::Png
        }
        fn can_decode(&self, f: ImageFormat) -> bool {
            f == ImageFormat::Png
        }
        fn encode(&self, _: &Bitmap, _: &EncodeOptions) -> Result<Vec<u8>, CodecError> {
            Ok(b"FAKE".to_vec())
        }
        fn decode(&self, _: ImageFormat, _: &[u8]) -> Result<Bitmap, CodecError> {
            Ok(one_pixel([9, 9, 9, 255]))
        }
    }

    #[test]
    fn extension_is_tried_before_builtins() {
        let img = gradient();
        let codecs = Codecs::builtin().with_extension(Arc::new(FakePng));
        assert!(codecs.can_encode(ImageFormat::Png));
        pool().install(|| {
            let real_png = encode(&img, ImageFormat::Png);
            assert_eq!(codecs.encode(&img, &opts(ImageFormat::Png)).unwrap(), b"FAKE");
            // The extension also takes over decoding of PNG bytes.
            assert_eq!(codecs.decode(&real_png).unwrap().pixels(), &[9, 9, 9, 255][..]);
            // JPEG is not claimed, so the built-in encoder runs.
            let jpeg = codecs.encode(&img, &opts(ImageFormat::Jpeg)).unwrap();
            assert_eq!(sniff(&jpeg), Some(ImageFormat::Jpeg));
        });
    }

    #[test]
    fn unknown_bytes_are_rejected() {
        assert!(matches!(Codecs::builtin().decode(b"not an image at all"), Err(CodecError::Unknown)));
    }

    #[test]
    fn truncated_input_does_not_panic() {
        let img = gradient();
        let codecs = Codecs::builtin();
        for format in ImageFormat::ALL {
            let bytes = pool().install(|| encode(&img, format));
            for cut in [1, bytes.len() / 2, bytes.len() - 1] {
                // Some decoders accept partial input; the only requirement here is no panic.
                let _ = codecs.decode(&bytes[..cut]);
            }
        }
    }

    #[test]
    fn empty_image_is_an_encode_error() {
        let empty = Bitmap::new(0, 0, PixelFormat::Rgba8, Vec::new().into()).unwrap();
        pool().install(|| {
            for format in ImageFormat::ALL {
                assert!(matches!(Codecs::builtin().encode(&empty, &opts(format)), Err(CodecError::Encode(_))), "{format:?}");
            }
        });
    }
}
