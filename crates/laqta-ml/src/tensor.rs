//! Pixels to model inputs. Pure: no ONNX Runtime, no threads, no clock.
//!
//! The resampling is bilinear and runs on the calling thread. `laqta_imaging::resize` is not used
//! here because it must run inside the compute pool.

use laqta_domain::{Bitmap, PhysRect, PixelFormat};

/// A packed 8-bit image in B, G, R order. PP-OCR was trained on BGR, so the channels stay in that order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BgrImage {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

impl BgrImage {
    /// Builds an image from packed B, G, R bytes. Returns `None` if the length is not `w * h * 3`.
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Option<Self> {
        (data.len() == width as usize * height as usize * 3).then_some(BgrImage { width, height, data })
    }

    /// Copies a bitmap's colour channels into B, G, R order. Alpha is dropped.
    pub fn from_bitmap(bitmap: &Bitmap) -> Self {
        let bgra = bitmap.format() == PixelFormat::Bgra8;
        let mut data = Vec::with_capacity(bitmap.width() as usize * bitmap.height() as usize * 3);
        for px in bitmap.pixels().as_chunks::<4>().0 {
            if bgra {
                data.extend_from_slice(&[px[0], px[1], px[2]]);
            } else {
                data.extend_from_slice(&[px[2], px[1], px[0]]);
            }
        }
        BgrImage { width: bitmap.width(), height: bitmap.height(), data }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    fn value(&self, x: u32, y: u32, channel: usize) -> f32 {
        let index = (y as usize * self.width as usize + x as usize) * 3 + channel;
        f32::from(self.data[index])
    }

    /// Bilinear sample at a fractional source position, clamped to the image.
    fn sample(&self, x: f32, y: f32, channel: usize) -> f32 {
        let max_x = (self.width - 1) as f32;
        let max_y = (self.height - 1) as f32;
        let x = x.clamp(0.0, max_x);
        let y = y.clamp(0.0, max_y);
        let (x0, y0) = (x.floor(), y.floor());
        let (x1, y1) = ((x0 + 1.0).min(max_x), (y0 + 1.0).min(max_y));
        let (fx, fy) = (x - x0, y - y0);
        let (x0, y0, x1, y1) = (x0 as u32, y0 as u32, x1 as u32, y1 as u32);
        let top = self.value(x0, y0, channel) * (1.0 - fx) + self.value(x1, y0, channel) * fx;
        let bottom = self.value(x0, y1, channel) * (1.0 - fx) + self.value(x1, y1, channel) * fx;
        top * (1.0 - fy) + bottom * fy
    }
}

/// Per-channel mean and standard deviation, applied in B, G, R order to values scaled to 0..1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Normalise {
    pub mean: [f32; 3],
    pub std: [f32; 3],
}

/// ImageNet statistics, used by the PP-OCR detector.
pub const IMAGENET: Normalise = Normalise { mean: [0.485, 0.456, 0.406], std: [0.229, 0.224, 0.225] };

/// `(x - 0.5) / 0.5`, used by the PP-OCR recogniser.
pub const HALF: Normalise = Normalise { mean: [0.5; 3], std: [0.5; 3] };

/// Clips a rectangle to the image. Returns `None` if nothing is left.
pub fn clip_rect(image: &BgrImage, rect: PhysRect) -> Option<PhysRect> {
    let x0 = i64::from(rect.x).clamp(0, i64::from(image.width));
    let y0 = i64::from(rect.y).clamp(0, i64::from(image.height));
    let x1 = (i64::from(rect.x) + i64::from(rect.width)).clamp(0, i64::from(image.width));
    let y1 = (i64::from(rect.y) + i64::from(rect.height)).clamp(0, i64::from(image.height));
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some(PhysRect { x: x0 as i32, y: y0 as i32, width: (x1 - x0) as u32, height: (y1 - y0) as u32 })
}

/// Resamples `region` of `src` to `dst_width` x `dst_height` and returns it as a flat NCHW buffer
/// (`[3, dst_height, dst_width]`), normalised per channel. The region must already be inside the
/// image; see `clip_rect`.
pub fn resample_nchw(src: &BgrImage, region: PhysRect, dst_width: u32, dst_height: u32, norm: Normalise) -> Vec<f32> {
    let (dw, dh) = (dst_width as usize, dst_height as usize);
    let mut out = vec![0.0; 3 * dw * dh];
    if src.width == 0 || src.height == 0 || dst_width == 0 || dst_height == 0 {
        return out;
    }
    let scale_x = region.width as f32 / dst_width as f32;
    let scale_y = region.height as f32 / dst_height as f32;
    for channel in 0..3 {
        let (mean, std) = (norm.mean[channel], norm.std[channel]);
        for dy in 0..dh {
            let sy = region.y as f32 + (dy as f32 + 0.5) * scale_y - 0.5;
            let row = (channel * dh + dy) * dw;
            for dx in 0..dw {
                let sx = region.x as f32 + (dx as f32 + 0.5) * scale_x - 0.5;
                let value = src.sample(sx, sy, channel) / 255.0;
                out[row + dx] = (value - mean) / std;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, width: u32, height: u32) -> PhysRect {
        PhysRect { x, y, width, height }
    }

    #[test]
    fn from_bitmap_reorders_rgba_to_bgr() {
        let bitmap = Bitmap::new(1, 1, PixelFormat::Rgba8, vec![10, 20, 30, 255].into()).expect("valid bitmap");
        let image = BgrImage::from_bitmap(&bitmap);
        assert_eq!(image.data, vec![30, 20, 10]);
    }

    #[test]
    fn from_bitmap_keeps_bgra_order() {
        let bitmap = Bitmap::new(1, 1, PixelFormat::Bgra8, vec![10, 20, 30, 255].into()).expect("valid bitmap");
        let image = BgrImage::from_bitmap(&bitmap);
        assert_eq!(image.data, vec![10, 20, 30]);
    }

    #[test]
    fn new_checks_length() {
        assert!(BgrImage::new(2, 2, vec![0; 12]).is_some());
        assert!(BgrImage::new(2, 2, vec![0; 11]).is_none());
    }

    #[test]
    fn clip_rect_trims_to_the_image() {
        let image = BgrImage::new(10, 10, vec![0; 300]).expect("valid image");
        assert_eq!(clip_rect(&image, rect(-2, 8, 5, 5)), Some(rect(0, 8, 3, 2)));
        assert_eq!(clip_rect(&image, rect(10, 0, 4, 4)), None);
    }

    #[test]
    fn identity_resample_keeps_values_with_half_normalisation() {
        // A 2x1 image: B=0 and B=255, G=R=0. Same size out, so each sample is its own pixel.
        let image = BgrImage::new(2, 1, vec![0, 0, 0, 255, 0, 0]).expect("valid image");
        let out = resample_nchw(&image, rect(0, 0, 2, 1), 2, 1, HALF);
        // Channel 0 (B): (0/255 - 0.5)/0.5 = -1 and (1 - 0.5)/0.5 = 1.
        assert!((out[0] + 1.0).abs() < 1e-5);
        assert!((out[1] - 1.0).abs() < 1e-5);
        // Channels 1 and 2 are 0 everywhere, which normalises to -1.
        assert!((out[2] + 1.0).abs() < 1e-5);
    }

    #[test]
    fn downscale_averages_neighbours() {
        // Two black pixels, then two pixels at 200. A 4x1 to 2x1 resample samples the middle of each pair.
        let data: Vec<u8> = [0u8, 0, 0, 0, 0, 0, 200, 200, 200, 200, 200, 200].to_vec();
        let image = BgrImage::new(4, 1, data).expect("valid image");
        let out = resample_nchw(&image, rect(0, 0, 4, 1), 2, 1, Normalise { mean: [0.0; 3], std: [1.0; 3] });
        assert!(out[0].abs() < 1e-5);
        assert!((out[1] - 200.0 / 255.0).abs() < 1e-5);
    }
}
