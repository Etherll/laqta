//! Pixel and file tools for the tool windows. Each tool is a pure function over `Bitmap`s or
//! encoded bytes. Functions that resize or encode run on the compute pool and assert that in debug
//! builds, as `ops::resize` does. Pixel outputs are Rgba8.

pub mod combine;
pub mod compare;
pub mod convert;
pub mod icon;
pub mod metadata;
pub mod split;
pub mod thumbnail;
pub mod watermark;

use laqta_domain::{Bitmap, PixelFormat};

/// Straight-alpha source-over of `src` onto `dst`. `opacity` (0..=1) scales the source alpha.
fn over(dst: [u8; 4], src: [u8; 4], opacity: f32) -> [u8; 4] {
    if opacity >= 1.0 && src[3] == 255 {
        return src;
    }
    let src_alpha = f32::from(src[3]) / 255.0 * opacity;
    if src_alpha <= 0.0 {
        return dst;
    }
    let dst_alpha = f32::from(dst[3]) / 255.0;
    let out_alpha = src_alpha + dst_alpha * (1.0 - src_alpha);
    let mix = |s: u8, d: u8| to_u8((f32::from(s) * src_alpha + f32::from(d) * dst_alpha * (1.0 - src_alpha)) / out_alpha);
    [mix(src[0], dst[0]), mix(src[1], dst[1]), mix(src[2], dst[2]), to_u8(out_alpha * 255.0)]
}

fn to_u8(v: f32) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

/// An Rgba8 bitmap over a pixel buffer that is already `width * height * 4` bytes long.
fn rgba_bitmap(width: u32, height: u32, pixels: Vec<u8>) -> Bitmap {
    Bitmap::new(width, height, PixelFormat::Rgba8, pixels.into()).unwrap_or_else(|e| unreachable!("pixel buffer built to size: {e:?}"))
}

/// The little-endian u16 at `at`, if the bytes are there.
fn le16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

/// The little-endian u32 at `at`, if the bytes are there.
fn le32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// The big-endian u32 at `at`, if the bytes are there.
fn be32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}
