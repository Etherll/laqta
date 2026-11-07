//! Rank and histogram filters over a square window: median and oil paint. The window clamps at the
//! edges, so every pixel sees `(2 * radius + 1)` squared samples. Alpha passes through unchanged.

use rayon::prelude::*;

use super::{Buf, window_index};

/// Per-channel median of the window. Radius 0 is identity.
pub(super) fn median(src: &Buf, radius: u32) -> Buf {
    if radius == 0 {
        return src.clone();
    }
    let r = radius as usize;
    let (w, h) = (src.w, src.h);
    let side = 2 * r + 1;
    let mut px = vec![0u8; src.px.len()];
    px.par_chunks_mut(w * 4).enumerate().for_each(|(y, out)| {
        let mut vals = Vec::with_capacity(side * side);
        for x in 0..w {
            for c in 0..3 {
                vals.clear();
                for dy in 0..side {
                    let sy = window_index(y, dy, r, h);
                    for dx in 0..side {
                        let sx = window_index(x, dx, r, w);
                        vals.push(src.px[(sy * w + sx) * 4 + c]);
                    }
                }
                let mid = vals.len() / 2;
                let (_, m, _) = vals.select_nth_unstable(mid);
                out[x * 4 + c] = *m;
            }
            out[x * 4 + 3] = src.px[(y * w + x) * 4 + 3];
        }
    });
    Buf { w, h, px }
}

/// Oil paint: in the window, pixels are binned by intensity into `levels` bins (1..=255), and the
/// most populated bin is replaced by its average colour. Ties go to the bin met first in reading order.
/// Radius 0 is identity.
pub(super) fn oil_paint(src: &Buf, radius: u32, levels: u8) -> Buf {
    if radius == 0 {
        return src.clone();
    }
    let r = radius as usize;
    let levels = usize::from(levels.clamp(1, 255));
    let (w, h) = (src.w, src.h);
    let side = 2 * r + 1;
    let mut px = vec![0u8; src.px.len()];
    px.par_chunks_mut(w * 4).enumerate().for_each(|(y, out)| {
        let mut count = vec![0u32; levels];
        let mut sum = vec![[0u32; 3]; levels];
        let mut touched: Vec<usize> = Vec::with_capacity(side * side);
        for x in 0..w {
            touched.clear();
            for dy in 0..side {
                let sy = window_index(y, dy, r, h);
                for dx in 0..side {
                    let sx = window_index(x, dx, r, w);
                    let i = (sy * w + sx) * 4;
                    let p = &src.px[i..i + 3];
                    let lum = (u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2])) / 3;
                    let bin = lum as usize * levels / 256;
                    if count[bin] == 0 {
                        touched.push(bin);
                    }
                    count[bin] += 1;
                    for (s, &v) in sum[bin].iter_mut().zip(p) {
                        *s += u32::from(v);
                    }
                }
            }
            let mut best = touched[0];
            for &b in &touched[1..] {
                if count[b] > count[best] {
                    best = b;
                }
            }
            let n = count[best];
            for (o, s) in out[x * 4..x * 4 + 3].iter_mut().zip(sum[best]) {
                *o = ((s + n / 2) / n) as u8;
            }
            out[x * 4 + 3] = src.px[(y * w + x) * 4 + 3];
            for &b in &touched {
                count[b] = 0;
                sum[b] = [0; 3];
            }
        }
    });
    Buf { w, h, px }
}
