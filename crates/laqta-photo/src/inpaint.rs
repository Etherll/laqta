//! The classic inpainting fallback, used when no learned model is installed (M7 D4). Telea-style:
//! masked pixels are filled from the boundary inward, each one as the distance-weighted average of
//! the known pixels around it.
//!
//! The fill order is a breadth-first wavefront from the known region, which approximates Telea's
//! fast marching. Weights are `1 / d^2` over a 9 x 9 window. The direction term of Telea's method
//! is left out, so the result is smooth rather than edge-aware.

use std::collections::VecDeque;

use laqta_domain::Bitmap;

use crate::mask::Mask;
use crate::surface::Surface;

/// Half-width of the averaging window, in pixels.
const RADIUS: i64 = 4;

/// Fills the pixels where `mask` is non-zero. Known pixels are the rest and are returned unchanged.
/// If `mask` does not match the bitmap's size, the bitmap is returned unchanged.
pub fn inpaint_fallback(bitmap: &Bitmap, mask: &Mask) -> Bitmap {
    let mut s = Surface::from_bitmap(bitmap);
    if mask.width() != s.w || mask.height() != s.h {
        return bitmap.clone();
    }
    let (w, h) = (s.w as usize, s.h as usize);
    let mut known: Vec<bool> = mask.data().iter().map(|&m| m == 0).collect();
    let mut queued = vec![false; w * h];
    let mut queue = VecDeque::new();
    // The wavefront starts at the unknown pixels that touch a known one.
    for (i, &is_known) in known.iter().enumerate() {
        if is_known {
            continue;
        }
        let (x, y) = (i % w, i / w);
        let touches_known = [(x.wrapping_sub(1), y), (x + 1, y), (x, y.wrapping_sub(1)), (x, y + 1)]
            .into_iter()
            .any(|(nx, ny)| nx < w && ny < h && known[ny * w + nx]);
        if touches_known {
            queued[i] = true;
            queue.push_back(i);
        }
    }
    while let Some(i) = queue.pop_front() {
        let (x, y) = ((i % w) as i64, (i / w) as i64);
        let mut acc = [0.0_f32; 4];
        let mut total = 0.0_f32;
        for ny in (y - RADIUS).max(0)..=(y + RADIUS).min(h as i64 - 1) {
            for nx in (x - RADIUS).max(0)..=(x + RADIUS).min(w as i64 - 1) {
                let j = ny as usize * w + nx as usize;
                if !known[j] {
                    continue;
                }
                let (dx, dy) = ((nx - x) as f32, (ny - y) as f32);
                let weight = 1.0 / (dx * dx + dy * dy);
                for (a, v) in acc.iter_mut().zip(&s.px[j * 4..j * 4 + 4]) {
                    *a += weight * f32::from(*v);
                }
                total += weight;
            }
        }
        // A queued pixel always has a known 4-neighbour, so `total` is positive.
        if total > 0.0 {
            for (dst, a) in s.px[i * 4..i * 4 + 4].iter_mut().zip(acc) {
                *dst = (a / total).round().clamp(0.0, 255.0) as u8;
            }
        }
        known[i] = true;
        let (x, y) = (i % w, i / w);
        let neighbours = [
            (x > 0).then(|| i - 1),
            (x + 1 < w).then(|| i + 1),
            (y > 0).then(|| i - w),
            (y + 1 < h).then(|| i + w),
        ];
        for n in neighbours.into_iter().flatten() {
            if !known[n] && !queued[n] {
                queued[n] = true;
                queue.push_back(n);
            }
        }
    }
    s.into_bitmap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mismatched_mask_returns_input() {
        let b = Surface::new(2, 2).into_bitmap();
        let m = Mask::filled(3, 3, 255);
        assert_eq!(inpaint_fallback(&b, &m).pixels(), b.pixels());
    }
}
