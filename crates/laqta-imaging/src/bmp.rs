//! Uncompressed BMP. Encodes 32-bit `BI_BITFIELDS` with alpha (BITMAPV4HEADER, bottom-up, BGRA
//! bytes). Decodes 24-bit and 32-bit `BI_RGB`, and 32-bit `BI_BITFIELDS` with the masks in the file.

use laqta_domain::{Bitmap, PixelFormat};

use crate::codecs::CodecError;

const FILE_HEADER_LEN: usize = 14;
const V4_HEADER_LEN: usize = 108;
const BI_RGB: u32 = 0;
const BI_BITFIELDS: u32 = 3;

pub(crate) fn encode(img: &Bitmap) -> Result<Vec<u8>, CodecError> {
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return Err(CodecError::Encode("empty image".into()));
    }
    let row_len = w as usize * 4;
    let data_len = row_len * h as usize;
    let pixel_offset = FILE_HEADER_LEN + V4_HEADER_LEN;
    let file_len = u32::try_from(pixel_offset + data_len).map_err(|_| CodecError::Encode("bmp larger than 4 GiB".into()))?;
    let width = i32::try_from(w).map_err(|_| CodecError::Encode("bmp width out of range".into()))?;
    let height = i32::try_from(h).map_err(|_| CodecError::Encode("bmp height out of range".into()))?;

    let mut out = Vec::with_capacity(file_len as usize);
    // BITMAPFILEHEADER
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&file_len.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(pixel_offset as u32).to_le_bytes());
    // BITMAPV4HEADER: positive height means bottom-up rows.
    out.extend_from_slice(&(V4_HEADER_LEN as u32).to_le_bytes());
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&BI_BITFIELDS.to_le_bytes());
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes()); // 72 dpi, horizontal
    out.extend_from_slice(&2835i32.to_le_bytes()); // 72 dpi, vertical
    out.extend_from_slice(&[0; 8]); // colours used, colours important
    out.extend_from_slice(&0x00FF_0000u32.to_le_bytes()); // red mask
    out.extend_from_slice(&0x0000_FF00u32.to_le_bytes()); // green mask
    out.extend_from_slice(&0x0000_00FFu32.to_le_bytes()); // blue mask
    out.extend_from_slice(&0xFF00_0000u32.to_le_bytes()); // alpha mask
    out.extend_from_slice(&0x7352_4742u32.to_le_bytes()); // LCS_sRGB
    out.extend_from_slice(&[0; 36]); // CIEXYZ endpoints (unused for sRGB)
    out.extend_from_slice(&[0; 12]); // gamma red, green, blue (unused for sRGB)

    let src = img.pixels();
    for y in (0..h as usize).rev() {
        let line = &src[y * row_len..(y + 1) * row_len];
        match img.format() {
            PixelFormat::Bgra8 => out.extend_from_slice(line),
            PixelFormat::Rgba8 => {
                for p in line.as_chunks::<4>().0 {
                    out.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
                }
            }
        }
    }
    Ok(out)
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Bitmap, CodecError> {
    let bad = |what: &str| CodecError::Decode(format!("bmp {what}"));
    if bytes.len() < 54 || &bytes[..2] != b"BM" {
        return Err(bad("signature or header"));
    }
    let pixel_offset = le_u32(bytes, 10).ok_or_else(|| bad("header"))? as usize;
    let dib_len = le_u32(bytes, 14).ok_or_else(|| bad("header"))?;
    if dib_len < 40 {
        return Err(bad("info header too small"));
    }
    let width = le_u32(bytes, 18).ok_or_else(|| bad("header"))? as i32;
    let height = le_u32(bytes, 22).ok_or_else(|| bad("header"))? as i32;
    let bpp = le_u16(bytes, 28).ok_or_else(|| bad("header"))?;
    let compression = le_u32(bytes, 30).ok_or_else(|| bad("header"))?;
    if width <= 0 || height == 0 {
        return Err(bad("size"));
    }
    let (w, h) = (width as usize, height.unsigned_abs() as usize);
    let top_down = height < 0;

    // Channel masks: BI_RGB uses the 32-bit default layout with no alpha.
    let masks = match (bpp, compression) {
        (24, BI_RGB) => None,
        (32, BI_RGB) => Some(Masks { red: 0x00FF_0000, green: 0x0000_FF00, blue: 0x0000_00FF, alpha: 0 }),
        (32, BI_BITFIELDS) => {
            let red = le_u32(bytes, 54).ok_or_else(|| bad("masks"))?;
            let green = le_u32(bytes, 58).ok_or_else(|| bad("masks"))?;
            let blue = le_u32(bytes, 62).ok_or_else(|| bad("masks"))?;
            // The alpha mask exists only in V3 and later headers; in a 40-byte header, offset 66 is pixel data.
            let alpha = if dib_len >= 56 { le_u32(bytes, 66).ok_or_else(|| bad("masks"))? } else { 0 };
            Some(Masks { red, green, blue, alpha })
        }
        _ => return Err(bad("only 24-bit or 32-bit uncompressed (BI_RGB) and 32-bit BI_BITFIELDS are supported")),
    };

    let stride = (w * bpp as usize).div_ceil(32) * 4;
    let needed = stride.checked_mul(h).ok_or_else(|| bad("size overflow"))?;
    let pixels = pixel_offset
        .checked_add(needed)
        .and_then(|end| bytes.get(pixel_offset..end))
        .ok_or_else(|| bad("pixel data is truncated"))?;

    let mut out = vec![0u8; w * h * 4];
    for row in 0..h {
        let src_row = if top_down { row } else { h - 1 - row };
        let src = &pixels[src_row * stride..src_row * stride + stride];
        let dst = &mut out[row * w * 4..(row + 1) * w * 4];
        for (x, d) in dst.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            *d = match masks {
                None => {
                    // 24-bit: bytes are B, G, R.
                    let p = &src[x * 3..x * 3 + 3];
                    [p[2], p[1], p[0], 255]
                }
                Some(m) => {
                    let v = u32::from_le_bytes(src[x * 4..x * 4 + 4].try_into().unwrap_or([0; 4]));
                    let alpha = if m.alpha == 0 { 255 } else { channel(v, m.alpha) };
                    [channel(v, m.red), channel(v, m.green), channel(v, m.blue), alpha]
                }
            };
        }
    }
    let width = u32::try_from(w).map_err(|_| bad("width"))?;
    let height = u32::try_from(h).map_err(|_| bad("height"))?;
    Bitmap::new(width, height, PixelFormat::Rgba8, out.into()).map_err(|e| CodecError::Decode(e.to_string()))
}

#[derive(Clone, Copy)]
struct Masks {
    red: u32,
    green: u32,
    blue: u32,
    alpha: u32,
}

/// Extracts one channel from a pixel and scales it to 0..=255.
fn channel(v: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let max = u64::from(mask >> shift);
    let value = u64::from((v & mask) >> shift);
    ((value * 255 + max / 2) / max) as u8
}

fn le_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn le_u16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn rgba(w: u32, h: u32, px: &[[u8; 4]]) -> Bitmap {
        assert_eq!(px.len(), (w * h) as usize);
        Bitmap::new(w, h, PixelFormat::Rgba8, px.concat().into()).unwrap()
    }

    #[test]
    fn round_trip_keeps_alpha_and_orientation() {
        let px = [[1, 2, 3, 255], [4, 5, 6, 0], [7, 8, 9, 128], [10, 11, 12, 255], [13, 14, 15, 64], [16, 17, 18, 255]];
        let img = rgba(3, 2, &px);
        let out = decode(&encode(&img).unwrap()).unwrap();
        assert_eq!((out.width(), out.height()), (3, 2));
        assert_eq!(out.pixels(), img.pixels());
    }

    #[test]
    fn decodes_24_bit_bottom_up() {
        // 2x2, 24-bit BI_RGB: stride is 8 (6 bytes of pixels + 2 padding). First stored row is the bottom.
        let mut file = Vec::new();
        file.extend_from_slice(b"BM");
        file.extend_from_slice(&70u32.to_le_bytes());
        file.extend_from_slice(&[0; 4]);
        file.extend_from_slice(&54u32.to_le_bytes());
        file.extend_from_slice(&40u32.to_le_bytes());
        file.extend_from_slice(&2i32.to_le_bytes());
        file.extend_from_slice(&2i32.to_le_bytes());
        file.extend_from_slice(&1u16.to_le_bytes());
        file.extend_from_slice(&24u16.to_le_bytes());
        file.extend_from_slice(&[0; 24]); // compression 0, rest of the info header
        // Bottom row (image row 1): blue, then grey.
        file.extend_from_slice(&[255, 0, 0, 9, 9, 9, 0, 0]);
        // Top row (image row 0): red, then green.
        file.extend_from_slice(&[0, 0, 255, 0, 255, 0, 0, 0]);
        let out = decode(&file).unwrap();
        assert_eq!(out.pixels(), &[255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 9, 9, 9, 255][..]);
    }

    #[test]
    fn decodes_32_bit_bi_rgb_with_opaque_alpha() {
        let mut file = Vec::new();
        file.extend_from_slice(b"BM");
        file.extend_from_slice(&58u32.to_le_bytes());
        file.extend_from_slice(&[0; 4]);
        file.extend_from_slice(&54u32.to_le_bytes());
        file.extend_from_slice(&40u32.to_le_bytes());
        file.extend_from_slice(&1i32.to_le_bytes());
        file.extend_from_slice(&(-1i32).to_le_bytes()); // top-down
        file.extend_from_slice(&1u16.to_le_bytes());
        file.extend_from_slice(&32u16.to_le_bytes());
        file.extend_from_slice(&[0; 24]);
        file.extend_from_slice(&[30, 20, 10, 0]); // B, G, R, unused
        let out = decode(&file).unwrap();
        assert_eq!(out.pixels(), &[10, 20, 30, 255][..]);
    }

    #[test]
    fn truncated_or_unsupported_input_is_an_error() {
        let bytes = encode(&rgba(2, 2, &[[0, 0, 0, 255]; 4])).unwrap();
        assert!(matches!(decode(&bytes[..bytes.len() - 1]), Err(CodecError::Decode(_))));
        assert!(matches!(decode(&bytes[..30]), Err(CodecError::Decode(_))));
        assert!(matches!(decode(b"BM"), Err(CodecError::Decode(_))));
        // 8-bit is not supported.
        let mut file = bytes.clone();
        file[28] = 8;
        assert!(matches!(decode(&file), Err(CodecError::Decode(_))));
    }
}
