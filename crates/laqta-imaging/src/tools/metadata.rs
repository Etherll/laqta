//! Metadata of image files: reading EXIF, and stripping metadata from JPEG, PNG and WebP.

use std::io::Cursor;

use laqta_domain::ImageFormat;

use super::{be32, le32};
use crate::{CodecError, sniff};

/// The EXIF fields of an image as `(tag, value)` pairs, in file order. Empty when the file has no EXIF data or is not
/// a container the reader understands (JPEG, PNG, WebP, TIFF or HEIF).
pub fn read_exif(bytes: &[u8]) -> Vec<(String, String)> {
    let Ok(data) = exif::Reader::new().read_from_container(&mut Cursor::new(bytes)) else {
        return Vec::new();
    };
    data.fields().map(|f| (f.tag.to_string(), f.display_value().with_unit(&data).to_string())).collect()
}

/// A copy of an image file without its metadata.
///
/// - JPEG: drops APP1 to APP15 and comments, except the JFIF header (APP0), the ICC colour profile (APP2) and the
///   Adobe colour marker (APP14). The last two change how pixels are read, so they stay.
/// - PNG: drops `tEXt`, `zTXt`, `iTXt`, `eXIf` and `tIME` chunks, and any bytes after `IEND`.
/// - WebP: drops `EXIF` and `XMP ` chunks, and clears their flags in the `VP8X` chunk.
///
/// Other formats give `Unsupported`, and bytes that are not an image give `Unknown`.
pub fn strip_metadata(bytes: &[u8]) -> Result<Vec<u8>, CodecError> {
    match sniff(bytes) {
        Some(ImageFormat::Jpeg) => strip_jpeg(bytes),
        Some(ImageFormat::Png) => strip_png(bytes),
        Some(ImageFormat::WebP) => strip_webp(bytes),
        Some(other) => Err(CodecError::Unsupported(other)),
        None => Err(CodecError::Unknown),
    }
}

fn truncated(what: &str) -> CodecError {
    CodecError::Decode(format!("{what} is truncated"))
}

fn strip_jpeg(bytes: &[u8]) -> Result<Vec<u8>, CodecError> {
    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(&bytes[..2]);
    let mut i = 2;
    loop {
        while bytes.get(i) == Some(&0xFF) && bytes.get(i + 1) == Some(&0xFF) {
            i += 1;
        }
        let (Some(&0xFF), Some(&code)) = (bytes.get(i), bytes.get(i + 1)) else {
            return Err(truncated("jpeg"));
        };
        match code {
            // End of image or start of scan: the rest is entropy-coded data, copied as it is.
            0xD9 | 0xDA => {
                out.extend_from_slice(&bytes[i..]);
                return Ok(out);
            }
            // Standalone markers with no length: temporary, restart and start of image.
            0x01 | 0xD0..=0xD8 => {
                out.extend_from_slice(&bytes[i..i + 2]);
                i += 2;
            }
            _ => {
                let len = bytes.get(i + 2..i + 4).map(|b| usize::from(u16::from_be_bytes([b[0], b[1]]))).unwrap_or(0);
                let end = i + 2 + len;
                if len < 2 || end > bytes.len() {
                    return Err(truncated("jpeg segment"));
                }
                let segment = &bytes[i..end];
                if keep_jpeg_segment(code, &segment[4..]) {
                    out.extend_from_slice(segment);
                }
                i = end;
            }
        }
    }
}

/// Whether a JPEG marker segment is kept: everything except the APPn and comment segments that carry metadata.
fn keep_jpeg_segment(code: u8, payload: &[u8]) -> bool {
    match code {
        0xE0 => true,
        0xE2 => payload.starts_with(b"ICC_PROFILE\0"),
        0xEE => payload.starts_with(b"Adobe"),
        0xE1..=0xEF | 0xFE => false,
        _ => true,
    }
}

fn strip_png(bytes: &[u8]) -> Result<Vec<u8>, CodecError> {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    const DROPPED: [&[u8]; 5] = [b"tEXt", b"zTXt", b"iTXt", b"eXIf", b"tIME"];

    let mut out = SIGNATURE.to_vec();
    let mut i = SIGNATURE.len();
    loop {
        let len = be32(bytes, i).ok_or_else(|| truncated("png chunk"))? as usize;
        let kind = bytes.get(i + 4..i + 8).ok_or_else(|| truncated("png chunk"))?;
        let end = i.checked_add(12 + len).filter(|&e| e <= bytes.len()).ok_or_else(|| truncated("png chunk"))?;
        if !DROPPED.contains(&kind) {
            out.extend_from_slice(&bytes[i..end]);
        }
        i = end;
        if kind == b"IEND" {
            return Ok(out);
        }
    }
}

fn strip_webp(bytes: &[u8]) -> Result<Vec<u8>, CodecError> {
    let riff_len = le32(bytes, 4).ok_or_else(|| truncated("webp"))? as usize;
    let end = (8 + riff_len).min(bytes.len());
    let mut out = b"RIFF\0\0\0\0WEBP".to_vec();
    let mut i = 12;
    while i < end {
        let size = le32(bytes, i + 4).ok_or_else(|| truncated("webp chunk"))? as usize;
        let data_end = i + 8 + size;
        if data_end > end {
            return Err(truncated("webp chunk"));
        }
        // Chunks are padded to an even length.
        let chunk_end = (data_end + (size & 1)).min(end);
        let kind = &bytes[i..i + 4];
        if kind != b"EXIF" && kind != b"XMP " {
            let start = out.len();
            out.extend_from_slice(&bytes[i..chunk_end]);
            if kind == b"VP8X" && size >= 1 {
                // Flags: EXIF is 0x08 and XMP is 0x04.
                out[start + 8] &= !(0x08 | 0x04);
            }
        }
        i = chunk_end;
    }
    let riff = u32::try_from(out.len() - 8).map_err(|_| CodecError::Encode("webp is too large".into()))?;
    out[4..8].copy_from_slice(&riff.to_le_bytes());
    Ok(out)
}
