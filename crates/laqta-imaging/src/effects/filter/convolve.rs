//! 3x3 convolutions: the matrix filter and the fixed kernels built on it. Colour is convolved on
//! straight channels, and alpha passes through unchanged.

use rayon::prelude::*;

use super::{Buf, to_u8, window_index};

pub(super) const SHARPEN: [f32; 9] = [0.0, -1.0, 0.0, -1.0, 5.0, -1.0, 0.0, -1.0, 0.0];
pub(super) const SMOOTH: [f32; 9] = [1.0, 2.0, 1.0, 2.0, 4.0, 2.0, 1.0, 2.0, 1.0];
pub(super) const EDGE_DETECT: [f32; 9] = [-1.0, -1.0, -1.0, -1.0, 8.0, -1.0, -1.0, -1.0, -1.0];
pub(super) const EMBOSS: [f32; 9] = [-2.0, -1.0, 0.0, -1.0, 1.0, 1.0, 0.0, 1.0, 2.0];
pub(super) const MEAN_REMOVAL: [f32; 9] = [-1.0, -1.0, -1.0, -1.0, 9.0, -1.0, -1.0, -1.0, -1.0];

/// `sum(kernel * neighbour) / divisor + offset` per colour channel, clamped to 0..=255. The kernel is
/// row-major from the top left. A divisor of 0 counts as 1.
pub(super) fn convolve(src: &Buf, kernel: &[f32; 9], divisor: f32, offset: f32) -> Buf {
    let divisor = if divisor == 0.0 { 1.0 } else { divisor };
    let (w, h) = (src.w, src.h);
    let mut px = vec![0u8; src.px.len()];
    px.par_chunks_mut(w * 4).enumerate().for_each(|(y, out)| {
        for x in 0..w {
            let mut acc = [0f32; 3];
            for (k, &weight) in kernel.iter().enumerate() {
                let sx = window_index(x, k % 3, 1, w);
                let sy = window_index(y, k / 3, 1, h);
                let i = (sy * w + sx) * 4;
                for (a, &v) in acc.iter_mut().zip(&src.px[i..i + 3]) {
                    *a += weight * f32::from(v);
                }
            }
            for (o, a) in out[x * 4..x * 4 + 3].iter_mut().zip(acc) {
                *o = to_u8(a / divisor + offset);
            }
            out[x * 4 + 3] = src.px[(y * w + x) * 4 + 3];
        }
    });
    Buf { w, h, px }
}
