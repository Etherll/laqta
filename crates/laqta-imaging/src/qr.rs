//! QR code encoding (after-upload "Show QR code", the QR tool).

use laqta_domain::{Bitmap, PixelFormat};

#[derive(Debug, Clone, thiserror::Error)]
#[error("qr: {0}")]
pub struct QrError(pub String);

/// Renders `text` as a black-on-white QR code, `module` pixels per module, with a 4-module quiet zone.
pub fn qr_encode(text: &str, module: u32) -> Result<Bitmap, QrError> {
    let code = qrcode::QrCode::new(text.as_bytes()).map_err(|e| QrError(e.to_string()))?;
    let n = code.width() as u32;
    let module = module.max(1);
    let quiet = 4;
    let size = (n + 2 * quiet) * module;
    let colors = code.to_colors();
    let mut px = vec![255u8; (size * size * 4) as usize];
    for (i, c) in colors.iter().enumerate() {
        if *c != qrcode::Color::Dark {
            continue;
        }
        let (mx, my) = (i as u32 % n + quiet, i as u32 / n + quiet);
        for y in my * module..(my + 1) * module {
            for x in mx * module..(mx + 1) * module {
                let o = ((y * size + x) * 4) as usize;
                px[o..o + 3].copy_from_slice(&[0, 0, 0]);
            }
        }
    }
    Bitmap::new(size, size, PixelFormat::Rgba8, px.into()).map_err(|e| QrError(e.to_string()))
}

/// Every barcode/QR code found in the image, by text (the QR tool, "scan QR code" after capture).
pub fn qr_decode(img: &Bitmap) -> Vec<String> {
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let luma: Vec<u8> = img
        .pixels()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| {
            let (r, g, b) = match img.format() {
                PixelFormat::Rgba8 => (p[0], p[1], p[2]),
                PixelFormat::Bgra8 => (p[2], p[1], p[0]),
            };
            // Transparent pixels read as white, like a page.
            let y = (299 * r as u32 + 587 * g as u32 + 114 * b as u32) / 1000;
            (255 - ((255 - y) * p[3] as u32 / 255)) as u8
        })
        .collect();
    match rxing::helpers::detect_multiple_in_luma(luma, w, h) {
        Ok(results) => results.iter().map(|r| r.getText().to_string()).collect(),
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn encodes_with_quiet_zone() {
        let b = qr_encode("https://example.com/a", 3).unwrap();
        assert_eq!(b.width(), b.height());
        assert_eq!(&b.pixels()[0..4], &[255, 255, 255, 255]); // quiet zone is white
        assert!(b.pixels().as_chunks::<4>().0.iter().any(|p| p[0] == 0));
    }

    #[test]
    fn decode_round_trip() {
        let b = qr_encode("https://example.com/qr", 4).unwrap();
        assert_eq!(qr_decode(&b), vec!["https://example.com/qr".to_string()]);
        let blank = Bitmap::new(8, 8, PixelFormat::Rgba8, vec![255; 256].into()).unwrap();
        assert!(qr_decode(&blank).is_empty());
    }
}
