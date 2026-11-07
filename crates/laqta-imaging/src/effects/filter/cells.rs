//! Square-cell effects: pixelate and halftone. Both work on a grid of `cell` x `cell` blocks, with
//! edge blocks cut short.

use rayon::prelude::*;

use super::Buf;

/// Each block becomes its average colour. The average is weighted by alpha, so transparent pixels
/// don't darken the block. Size 1 (or 0) is identity.
pub(super) fn pixelate(src: &Buf, size: u32) -> Buf {
    let size = size.max(1) as usize;
    if size == 1 {
        return src.clone();
    }
    let (w, h) = (src.w, src.h);
    let (bw, bh) = (w.div_ceil(size), h.div_ceil(size));
    let blocks = block_averages(src, size, bw, bh);
    let mut px = vec![0u8; src.px.len()];
    px.par_chunks_mut(w * 4).enumerate().for_each(|(y, out)| {
        let by = y / size;
        for x in 0..w {
            let p = blocks[by * bw + x / size];
            out[x * 4..x * 4 + 4].copy_from_slice(&p);
        }
    });
    Buf { w, h, px }
}

/// Each `cell` x `cell` block becomes a white square with a dot in it, in the block's average colour.
/// The dot's area follows the block's darkness, so dark blocks get big dots and light ones small dots.
/// Alpha is kept.
pub(super) fn halftone(src: &Buf, cell: u32) -> Buf {
    let cell = cell.max(1) as usize;
    let (w, h) = (src.w, src.h);
    let (bw, bh) = (w.div_ceil(cell), h.div_ceil(cell));
    let blocks = block_averages(src, cell, bw, bh);
    let half = cell as f32 / 2.0;
    let mut px = vec![0u8; src.px.len()];
    px.par_chunks_mut(w * 4).enumerate().for_each(|(y, out)| {
        let by = y / cell;
        let cy = by as f32 * cell as f32 + half;
        for x in 0..w {
            let bx = x / cell;
            let block = blocks[by * bw + bx];
            let lum = (0.299 * f32::from(block[0]) + 0.587 * f32::from(block[1]) + 0.114 * f32::from(block[2])) / 255.0;
            let radius = half * (1.0 - lum).sqrt();
            let cx = bx as f32 * cell as f32 + half;
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            let inside = (dx * dx + dy * dy).sqrt() <= radius;
            let rgb = if inside { [block[0], block[1], block[2]] } else { [255, 255, 255] };
            let o = &mut out[x * 4..x * 4 + 4];
            o[..3].copy_from_slice(&rgb);
            o[3] = src.px[(y * w + x) * 4 + 3];
        }
    });
    Buf { w, h, px }
}

/// The alpha-weighted average colour and the mean alpha of every block, straight RGBA. Blocks are
/// row-major, `bw` wide. A block with no alpha at all comes out transparent black.
fn block_averages(src: &Buf, size: usize, bw: usize, bh: usize) -> Vec<[u8; 4]> {
    (0..bw * bh)
        .into_par_iter()
        .map(|b| {
            let (x0, y0) = ((b % bw) * size, (b / bw) * size);
            let (x1, y1) = ((x0 + size).min(src.w), (y0 + size).min(src.h));
            let mut weighted = [0u64; 3];
            let mut alpha = 0u64;
            let mut n = 0u64;
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = &src.px[(y * src.w + x) * 4..][..4];
                    let a = u64::from(p[3]);
                    for (s, &v) in weighted.iter_mut().zip(p) {
                        *s += u64::from(v) * a;
                    }
                    alpha += a;
                    n += 1;
                }
            }
            if alpha == 0 {
                return [0, 0, 0, 0];
            }
            let channel = |s: u64| ((s + alpha / 2) / alpha) as u8;
            [channel(weighted[0]), channel(weighted[1]), channel(weighted[2]), ((alpha + n / 2) / n) as u8]
        })
        .collect()
}
