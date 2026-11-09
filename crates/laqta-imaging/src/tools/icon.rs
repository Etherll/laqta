//! Multi-size ICO icons: encoding one image at several sizes, and decoding ICO files.

use laqta_domain::Bitmap;

use super::{le16, le32, rgba_bitmap};
use crate::ops;
use crate::{CodecError, Codecs, encode_png};

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// Encodes `img` as an ICO file with one entry per size in `sizes`. Each entry is a `s` x `s` square: the image is
/// scaled to fit, aspect kept, and centred on a transparent background. Entries are PNG-compressed. Each size must be
/// in 1..=256, and at most 65535 sizes are allowed. Runs on the compute pool.
pub fn encode_ico(img: &Bitmap, sizes: &[u32]) -> Result<Vec<u8>, CodecError> {
    debug_assert!(rayon::current_thread_index().is_some(), "encode_ico must run inside the compute pool");
    if sizes.is_empty() || u16::try_from(sizes.len()).is_err() {
        return Err(CodecError::Encode("an icon needs between 1 and 65535 sizes".into()));
    }
    if let Some(bad) = sizes.iter().find(|s| !(1..=256).contains(*s)) {
        return Err(CodecError::Encode(format!("icon size {bad} is outside 1..=256")));
    }
    if img.width() == 0 || img.height() == 0 {
        return Err(CodecError::Encode("empty image".into()));
    }
    let entries = sizes.iter().map(|&s| encode_entry(img, s)).collect::<Result<Vec<_>, _>>()?;

    let mut out = vec![0, 0, 1, 0];
    out.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len();
    for (&s, data) in sizes.iter().zip(&entries) {
        let dim = if s == 256 { 0 } else { s as u8 };
        let len = u32::try_from(data.len()).map_err(|_| CodecError::Encode("icon is too large".into()))?;
        let at = u32::try_from(offset).map_err(|_| CodecError::Encode("icon is too large".into()))?;
        out.extend_from_slice(&[dim, dim, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&at.to_le_bytes());
        offset += data.len();
    }
    for data in &entries {
        out.extend_from_slice(data);
    }
    Ok(out)
}

/// One `side` x `side` PNG: `img` scaled to fit and centred on a transparent square.
fn encode_entry(img: &Bitmap, side: u32) -> Result<Vec<u8>, CodecError> {
    let (iw, ih) = (f64::from(img.width()), f64::from(img.height()));
    let scale = (f64::from(side) / iw).min(f64::from(side) / ih);
    let w = ((iw * scale).round() as u32).clamp(1, side);
    let h = ((ih * scale).round() as u32).clamp(1, side);
    let rgba = ops::to_rgba(img);
    let scaled = if (w, h) == (rgba.width(), rgba.height()) { rgba } else { ops::resize(&rgba, w, h) };

    let (x0, y0) = (((side - w) / 2) as usize, ((side - h) / 2) as usize);
    let (square, line, src) = (side as usize, w as usize * 4, scaled.pixels());
    let mut canvas = vec![0u8; square * square * 4];
    for y in 0..h as usize {
        let d = ((y0 + y) * square + x0) * 4;
        canvas[d..d + line].copy_from_slice(&src[y * line..(y + 1) * line]);
    }
    let framed = rgba_bitmap(side, side, canvas);
    encode_png(&framed).map_err(|e| CodecError::Encode(e.to_string()))
}

/// Decodes every entry of an ICO file to Rgba8, in directory order. PNG entries and uncompressed 32-bit BMP entries
/// are read. For a BMP entry with no alpha, the AND mask gives the alpha, and an entry with no alpha and no mask is opaque.
pub fn decode_ico(bytes: &[u8]) -> Result<Vec<Bitmap>, CodecError> {
    if bytes.len() < 6 || bytes[..4] != [0, 0, 1, 0] {
        return Err(CodecError::Decode("not an icon file".into()));
    }
    let count = usize::from(le16(bytes, 4).unwrap_or(0));
    let mut images = Vec::with_capacity(count);
    for n in 0..count {
        let at = 6 + 16 * n;
        let (Some(size), Some(offset)) = (le32(bytes, at + 8), le32(bytes, at + 12)) else {
            return Err(CodecError::Decode("icon directory is truncated".into()));
        };
        let data = (offset as usize)
            .checked_add(size as usize)
            .and_then(|end| bytes.get(offset as usize..end))
            .ok_or_else(|| CodecError::Decode("icon entry is outside the file".into()))?;
        images.push(decode_entry(data)?);
    }
    Ok(images)
}

fn decode_entry(data: &[u8]) -> Result<Bitmap, CodecError> {
    if data.starts_with(PNG_SIGNATURE) {
        return Codecs::builtin().decode(data);
    }
    decode_dib(data)
}

/// A BITMAPINFOHEADER entry with 32-bit BI_RGB pixels. Its height counts the pixels and the AND mask, and both are
/// stored bottom-up.
fn decode_dib(data: &[u8]) -> Result<Bitmap, CodecError> {
    let bad = |msg: &str| CodecError::Decode(msg.into());
    let header = le32(data, 0).ok_or_else(|| bad("icon image header is truncated"))? as usize;
    let width = le32(data, 4).ok_or_else(|| bad("icon image header is truncated"))? as i32;
    let height_total = le32(data, 8).ok_or_else(|| bad("icon image header is truncated"))? as i32;
    let bits = le16(data, 14).unwrap_or(0);
    let compression = le32(data, 16).unwrap_or(1);
    if header < 40 || bits != 32 || compression != 0 || width <= 0 || height_total.unsigned_abs() < 2 {
        return Err(bad("only 32-bit uncompressed icon images are supported"));
    }
    let (width, height) = (width as usize, height_total.unsigned_abs() as usize / 2);
    let top_down = height_total < 0;
    let overflow = || bad("icon image is too large");
    let stride = width.checked_mul(4).ok_or_else(overflow)?;
    let xor_len = stride.checked_mul(height).ok_or_else(overflow)?;
    let xor = data.get(header..).and_then(|rest| rest.get(..xor_len)).ok_or_else(|| bad("icon pixels are truncated"))?;
    let mask_stride = width.div_ceil(32) * 4;
    let mask_len = mask_stride.checked_mul(height).ok_or_else(overflow)?;
    let mask = data.get(header + xor_len..).and_then(|rest| rest.get(..mask_len));

    let row_of = |y: usize| if top_down { y } else { height - 1 - y };
    let mut rgba = vec![0u8; width * height * 4];
    let mut has_alpha = false;
    for y in 0..height {
        let src_row = row_of(y);
        for x in 0..width {
            let s = src_row * stride + x * 4;
            let d = (y * width + x) * 4;
            rgba[d..d + 4].copy_from_slice(&[xor[s + 2], xor[s + 1], xor[s], xor[s + 3]]);
            has_alpha |= xor[s + 3] != 0;
        }
    }
    if !has_alpha {
        for y in 0..height {
            for x in 0..width {
                let opaque = match mask {
                    Some(m) => (m[row_of(y) * mask_stride + x / 8] >> (7 - x % 8)) & 1 == 0,
                    None => true,
                };
                rgba[(y * width + x) * 4 + 3] = if opaque { 255 } else { 0 };
            }
        }
    }
    Ok(rgba_bitmap(width as u32, height as u32, rgba))
}
