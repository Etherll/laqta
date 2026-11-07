//! Pixel operations on `Bitmap`: crop, resize, thumbnail and nearest-neighbour upscale. Output
//! keeps the input's pixel format.

use fast_image_resize::images::{Image, ImageRef};
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
use laqta_domain::{Bitmap, PhysRect, PixelFormat};

/// The part of `img` inside `rect` (image pixel coordinates), clamped to the image. An empty
/// intersection gives a 1x1 transparent bitmap.
pub fn crop(img: &Bitmap, rect: PhysRect) -> Bitmap {
    let (iw, ih) = (i64::from(img.width()), i64::from(img.height()));
    let x0 = i64::from(rect.x).clamp(0, iw);
    let y0 = i64::from(rect.y).clamp(0, ih);
    let x1 = (i64::from(rect.x) + i64::from(rect.width)).clamp(0, iw);
    let y1 = (i64::from(rect.y) + i64::from(rect.height)).clamp(0, ih);
    if x1 <= x0 || y1 <= y0 {
        return transparent_pixel(img.format());
    }
    let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
    let stride = img.width() as usize * 4;
    let src = img.pixels();
    let mut out = Vec::with_capacity(w * h * 4);
    for y in y0 as usize..y1 as usize {
        let start = y * stride + x0 as usize * 4;
        out.extend_from_slice(&src[start..start + w * 4]);
    }
    bitmap(w as u32, h as u32, img.format(), out)
}

/// Resamples to `width` x `height` with Lanczos3. A zero side is treated as 1. An empty source
/// gives a transparent image of the target size.
///
/// Runs on the compute pool (`fast_image_resize` splits rows over rayon).
pub fn resize(img: &Bitmap, width: u32, height: u32) -> Bitmap {
    debug_assert!(rayon::current_thread_index().is_some(), "resize must run inside the compute pool");
    let (w, h) = (width.max(1), height.max(1));
    if img.width() == 0 || img.height() == 0 {
        return bitmap(w, h, img.format(), vec![0; w as usize * h as usize * 4]);
    }
    // U8x4 is correct for RGBA and BGRA alike: alpha is the fourth byte in both.
    let src = ImageRef::new(img.width(), img.height(), img.pixels(), PixelType::U8x4)
        .unwrap_or_else(|e| unreachable!("bitmap buffer was checked on construction: {e:?}"));
    let mut dst = Image::new(w, h, PixelType::U8x4);
    let options = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3));
    if let Err(e) = Resizer::new().resize(&src, &mut dst, &options) {
        unreachable!("resize between two U8x4 images cannot fail: {e:?}");
    }
    bitmap(w, h, img.format(), dst.into_vec())
}

/// Scales down to fit `max_width` x `max_height`, keeping the aspect ratio. A bound of 0 means
/// unbounded on that axis. Never upscales: an image that already fits is returned as is. The result
/// is at least 1x1.
///
/// Only a downscale calls `resize`, so the compute pool is needed only then.
pub fn thumbnail(img: &Bitmap, max_width: u32, max_height: u32) -> Bitmap {
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return transparent_pixel(img.format());
    }
    let fits = (max_width == 0 || w <= max_width) && (max_height == 0 || h <= max_height);
    if fits {
        return img.clone();
    }
    // At least one bound is set and exceeded, so the scale is finite and below 1.
    let scale_for = |len: u32, bound: u32| if bound == 0 { f64::INFINITY } else { f64::from(bound) / f64::from(len) };
    let scale = scale_for(w, max_width).min(scale_for(h, max_height));
    let fitted = |len: u32, bound: u32| {
        let v = ((f64::from(len) * scale).round() as u32).max(1);
        if bound == 0 { v } else { v.min(bound) }
    };
    resize(img, fitted(w, max_width), fitted(h, max_height))
}

/// Each pixel becomes a `factor` x `factor` block. A factor of 0 is treated as 1.
///
/// # Panics
/// If the enlarged width or height does not fit in `u32`.
pub fn upscale_nearest(img: &Bitmap, factor: u32) -> Bitmap {
    let f = factor.max(1) as usize;
    let (w, h) = (img.width() as usize, img.height() as usize);
    let ow = u32::try_from(w * f).expect("upscaled width fits in u32");
    let oh = u32::try_from(h * f).expect("upscaled height fits in u32");
    let src = img.pixels();
    let row_len = ow as usize * 4;
    let mut out = vec![0u8; row_len * oh as usize];
    for (oy, dst_row) in out.chunks_exact_mut(row_len).enumerate() {
        let src_row = &src[(oy / f) * w * 4..(oy / f + 1) * w * 4];
        for (ox, d) in dst_row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let s = (ox / f) * 4;
            d.copy_from_slice(&src_row[s..s + 4]);
        }
    }
    bitmap(ow, oh, img.format(), out)
}

fn transparent_pixel(format: PixelFormat) -> Bitmap {
    bitmap(1, 1, format, vec![0; 4])
}

fn bitmap(width: u32, height: u32, format: PixelFormat, pixels: Vec<u8>) -> Bitmap {
    debug_assert_eq!(pixels.len(), width as usize * height as usize * 4);
    Bitmap::new(width, height, format, pixels.into()).unwrap_or_else(|e| unreachable!("pixel buffer built to size: {e:?}"))
}

/// The same image as Rgba8 (a cheap clone when it already is).
pub fn to_rgba(img: &Bitmap) -> Bitmap {
    if img.format() == laqta_domain::PixelFormat::Rgba8 {
        return img.clone();
    }
    let px: Vec<u8> = img.pixels().as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
    Bitmap::new(img.width(), img.height(), laqta_domain::PixelFormat::Rgba8, px.into()).unwrap_or_else(|_| img.clone())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn pool() -> rayon::ThreadPool {
        #[allow(clippy::disallowed_types)]
        rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap()
    }

    fn solid(w: u32, h: u32, format: PixelFormat, px: [u8; 4]) -> Bitmap {
        let pixels: Vec<u8> = std::iter::repeat_n(px, (w * h) as usize).flatten().collect();
        Bitmap::new(w, h, format, pixels.into()).unwrap()
    }

    /// Pixel (x, y) encodes its position: red = x, green = y.
    fn grid(w: u32, h: u32) -> Bitmap {
        let mut px = Vec::new();
        for y in 0..h {
            for x in 0..w {
                px.extend_from_slice(&[x as u8, y as u8, 0, 255]);
            }
        }
        Bitmap::new(w, h, PixelFormat::Rgba8, px.into()).unwrap()
    }

    fn at(b: &Bitmap, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * b.width() + x) * 4) as usize;
        b.pixels()[i..i + 4].try_into().unwrap()
    }

    fn rect(x: i32, y: i32, width: u32, height: u32) -> PhysRect {
        PhysRect { x, y, width, height }
    }

    #[test]
    fn crop_takes_the_inside_rect() {
        let img = grid(4, 4);
        let out = crop(&img, rect(1, 2, 2, 2));
        assert_eq!((out.width(), out.height()), (2, 2));
        assert_eq!(at(&out, 0, 0), [1, 2, 0, 255]);
        assert_eq!(at(&out, 1, 1), [2, 3, 0, 255]);
    }

    #[test]
    fn crop_clamps_to_the_image() {
        let img = grid(4, 4);
        let out = crop(&img, rect(-1, -1, 3, 3));
        assert_eq!((out.width(), out.height()), (2, 2));
        assert_eq!(at(&out, 0, 0), [0, 0, 0, 255]);
        let out = crop(&img, rect(3, 2, 10, 10));
        assert_eq!((out.width(), out.height()), (1, 2));
        assert_eq!(at(&out, 0, 1), [3, 3, 0, 255]);
    }

    #[test]
    fn crop_outside_is_one_transparent_pixel() {
        let img = grid(4, 4);
        for r in [rect(10, 10, 2, 2), rect(0, 0, 0, 5), rect(-9, 0, 4, 4)] {
            let out = crop(&img, r);
            assert_eq!((out.width(), out.height()), (1, 1), "{r:?}");
            assert_eq!(at(&out, 0, 0), [0, 0, 0, 0], "{r:?}");
        }
    }

    #[test]
    fn crop_keeps_bgra() {
        let img = solid(3, 3, PixelFormat::Bgra8, [1, 2, 3, 4]);
        assert_eq!(crop(&img, rect(0, 0, 2, 2)).format(), PixelFormat::Bgra8);
    }

    #[test]
    fn resize_sets_size_and_keeps_colour() {
        let img = solid(8, 8, PixelFormat::Bgra8, [10, 20, 30, 255]);
        let out = pool().install(|| resize(&img, 4, 3));
        assert_eq!((out.width(), out.height()), (4, 3));
        assert_eq!(out.format(), PixelFormat::Bgra8);
        for p in out.pixels().as_chunks::<4>().0.iter() {
            assert!(p.iter().zip([10u8, 20, 30, 255]).all(|(a, b)| a.abs_diff(b) <= 1), "{p:?}");
        }
    }

    #[test]
    fn resize_zero_target_side_is_one() {
        let img = grid(4, 4);
        let out = pool().install(|| resize(&img, 0, 2));
        assert_eq!((out.width(), out.height()), (1, 2));
    }

    #[test]
    fn thumbnail_bounds_and_aspect() {
        let img = solid(100, 50, PixelFormat::Rgba8, [1, 2, 3, 255]);
        pool().install(|| {
            let dims = |out: Bitmap| (out.width(), out.height());
            assert_eq!(dims(thumbnail(&img, 20, 0)), (20, 10));
            assert_eq!(dims(thumbnail(&img, 0, 10)), (20, 10));
            assert_eq!(dims(thumbnail(&img, 30, 30)), (30, 15));
            assert_eq!(dims(thumbnail(&img, 1, 1)), (1, 1));
        });
    }

    #[test]
    fn thumbnail_never_upscales() {
        let img = solid(100, 50, PixelFormat::Rgba8, [1, 2, 3, 255]);
        // No downscale is needed, so this runs outside the compute pool.
        for (w, h) in [(200, 200), (0, 0), (100, 0), (0, 50)] {
            let out = thumbnail(&img, w, h);
            assert_eq!((out.width(), out.height()), (100, 50), "bounds {w}x{h}");
        }
    }

    #[test]
    fn thumbnail_of_empty_is_one_transparent_pixel() {
        let empty = Bitmap::new(0, 0, PixelFormat::Rgba8, Vec::new().into()).unwrap();
        let out = thumbnail(&empty, 10, 10);
        assert_eq!((out.width(), out.height()), (1, 1));
        assert_eq!(at(&out, 0, 0), [0, 0, 0, 0]);
    }

    #[test]
    fn upscale_nearest_repeats_pixels() {
        let img = grid(2, 1);
        let out = upscale_nearest(&img, 2);
        assert_eq!((out.width(), out.height()), (4, 2));
        assert_eq!(at(&out, 0, 0), [0, 0, 0, 255]);
        assert_eq!(at(&out, 1, 0), [0, 0, 0, 255]);
        assert_eq!(at(&out, 2, 0), [1, 0, 0, 255]);
        assert_eq!(at(&out, 3, 1), [1, 0, 0, 255]);
    }

    #[test]
    fn upscale_factor_zero_is_identity() {
        let img = grid(3, 2);
        let out = upscale_nearest(&img, 0);
        assert_eq!(out.pixels(), img.pixels());
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "compute pool")]
    fn resize_panics_outside_pool() {
        let _ = resize(&grid(2, 2), 1, 1);
    }
}
