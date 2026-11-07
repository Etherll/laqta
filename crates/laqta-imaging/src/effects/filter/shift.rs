//! Pixel-moving effects: RGB split and slice. Both keep the canvas size.

use rayon::prelude::*;

use super::{Buf, Rng, shifted};

/// Moves red, green and blue by their own (x, y) offsets. A channel that would come from outside the
/// image takes the nearest edge pixel. Alpha stays where it was.
pub(super) fn rgb_split(src: &Buf, offsets: [(i32, i32); 3]) -> Buf {
    let (w, h) = (src.w, src.h);
    let mut px = vec![0u8; src.px.len()];
    px.par_chunks_mut(w * 4).enumerate().for_each(|(y, out)| {
        for x in 0..w {
            let mut p = [0u8; 4];
            for (c, (dx, dy)) in offsets.into_iter().enumerate() {
                let sx = shifted(x, i64::from(dx), w);
                let sy = shifted(y, i64::from(dy), h);
                p[c] = src.px[(sy * w + sx) * 4 + c];
            }
            p[3] = src.px[(y * w + x) * 4 + 3];
            out[x * 4..x * 4 + 4].copy_from_slice(&p);
        }
    });
    Buf { w, h, px }
}

/// Horizontal strips, each `min_height..=max_height` tall (at least 1), shifted left or right by
/// `min_shift..=max_shift` pixels. Pixels shifted in from outside the image are transparent. The
/// strips come from one seeded stream, drawn from the top down.
pub(super) fn slice(src: &Buf, min_height: u32, max_height: u32, min_shift: u32, max_shift: u32, seed: u64) -> Buf {
    let (w, h) = (src.w, src.h);
    let mut rng = Rng::new(seed);
    let mut row_shift = vec![0i64; h];
    let mut y = 0;
    while y < h {
        let height = rng.range(min_height.max(1), max_height.max(1)) as usize;
        let amount = i64::from(rng.range(min_shift, max_shift));
        let shift = if rng.next_u64() & 1 == 0 { amount } else { -amount };
        let end = (y + height).min(h);
        row_shift[y..end].fill(shift);
        y = end;
    }
    let mut px = vec![0u8; src.px.len()];
    px.par_chunks_mut(w * 4).enumerate().for_each(|(y, out)| {
        let shift = row_shift[y];
        for x in 0..w {
            let sx = x as i64 - shift;
            if (0..w as i64).contains(&sx) {
                let i = (y * w + sx as usize) * 4;
                out[x * 4..x * 4 + 4].copy_from_slice(&src.px[i..i + 4]);
            }
        }
    });
    Buf { w, h, px }
}
