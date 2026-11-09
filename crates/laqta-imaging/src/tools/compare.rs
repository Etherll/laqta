//! Pixel comparison of two images.

use laqta_domain::Bitmap;

use super::rgba_bitmap;
use crate::ops;

/// The result of `compare`.
#[derive(Clone, Debug)]
pub struct Diff {
    /// A copy of `a` composited over black and dimmed, over the union of both sizes, with changed pixels in red.
    pub image: Bitmap,
    /// Pixels of the union that differ by more than the tolerance, plus every union pixel that only one image covers.
    pub changed: u64,
    /// Pixels in the union of both sizes (the larger width times the larger height).
    pub total: u64,
    /// The largest difference of any channel (RGBA) over the overlap.
    pub max_delta: u8,
    /// Peak signal-to-noise ratio over the overlap, in dB, over all four channels. Infinite when the overlap is
    /// identical, and 0 when the images share no pixels.
    pub psnr: f64,
}

const RED: [u8; 4] = [255, 0, 0, 255];

/// Compares `a` and `b` pixel by pixel from the top-left corner. A pixel counts as changed when its largest channel
/// difference is greater than `tolerance`. Images of different sizes are compared over their overlap. Every other
/// pixel of the union counts as changed. Both images are read as Rgba8, so the pixel format does not matter.
pub fn compare(a: &Bitmap, b: &Bitmap, tolerance: u8) -> Diff {
    let (a, b) = (ops::to_rgba(a), ops::to_rgba(b));
    let (aw, ah) = (a.width() as usize, a.height() as usize);
    let (bw, bh) = (b.width() as usize, b.height() as usize);
    let (w, h) = (aw.max(bw), ah.max(bh));
    let (out_w, out_h) = (w.max(1), h.max(1));
    let (pa, pb) = (a.pixels().as_chunks::<4>().0, b.pixels().as_chunks::<4>().0);

    let mut out = vec![[0u8, 0, 0, 0]; out_w * out_h];
    let (mut changed, mut max_delta, mut sq_sum, mut overlap) = (0u64, 0u8, 0u64, 0u64);
    for y in 0..h {
        for x in 0..w {
            let o = y * out_w + x;
            if x < aw && y < ah && x < bw && y < bh {
                let (ea, eb) = (pa[y * aw + x], pb[y * bw + x]);
                let deltas = [0, 1, 2, 3].map(|c| ea[c].abs_diff(eb[c]));
                let delta = deltas.into_iter().max().unwrap_or(0);
                overlap += 1;
                max_delta = max_delta.max(delta);
                sq_sum += deltas.iter().map(|&d| u64::from(d) * u64::from(d)).sum::<u64>();
                if delta > tolerance {
                    changed += 1;
                    out[o] = RED;
                } else {
                    out[o] = dim(ea);
                }
            } else {
                changed += 1;
                out[o] = RED;
            }
        }
    }

    let psnr = if overlap == 0 {
        0.0
    } else if sq_sum == 0 {
        f64::INFINITY
    } else {
        let mse = sq_sum as f64 / (overlap * 4) as f64;
        10.0 * (255.0_f64 * 255.0 / mse).log10()
    };
    let pixels = out.into_flattened();
    Diff { image: rgba_bitmap(out_w as u32, out_h as u32, pixels), changed, total: (w * h) as u64, max_delta, psnr }
}

/// A pixel composited over black and dimmed to 35%, so changed pixels stand out.
fn dim(p: [u8; 4]) -> [u8; 4] {
    let alpha = u32::from(p[3]);
    let shade = |c: u8| (u32::from(c) * alpha / 255 * 35 / 100) as u8;
    [shade(p[0]), shade(p[1]), shade(p[2]), 255]
}
