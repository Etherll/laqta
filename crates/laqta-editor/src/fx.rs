//! Pixel work for the effect annotations (Blur, Pixelate, Highlight, Smart eraser, Magnify, Clone)
//! and whole-image operations (rotate, flip, cut out, auto crop). Straight-alpha RGBA buffers.

use laqta_domain::color::Rgba;
use laqta_domain::geometry::PhysRect;
use laqta_domain::{Bitmap, PixelFormat};

/// A straight-alpha RGBA image owned as a Vec, for in-place work.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Img {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

impl Img {
    pub fn new(w: u32, h: u32) -> Img {
        Img { w, h, px: vec![0; (w as usize) * (h as usize) * 4] }
    }

    pub fn from_bitmap(b: &Bitmap) -> Img {
        let px = match b.format() {
            PixelFormat::Rgba8 => b.pixels().to_vec(),
            PixelFormat::Bgra8 => b.pixels().as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0], p[3]]).collect(),
        };
        Img { w: b.width(), h: b.height(), px }
    }

    pub fn to_bitmap(&self) -> Bitmap {
        Bitmap::new(self.w, self.h, PixelFormat::Rgba8, self.px.clone().into()).unwrap_or_else(|_| empty_bitmap())
    }

    pub fn get(&self, x: i32, y: i32) -> [u8; 4] {
        let x = x.clamp(0, self.w as i32 - 1) as usize;
        let y = y.clamp(0, self.h as i32 - 1) as usize;
        let i = (y * self.w as usize + x) * 4;
        [self.px[i], self.px[i + 1], self.px[i + 2], self.px[i + 3]]
    }

    pub fn set(&mut self, x: u32, y: u32, p: [u8; 4]) {
        let i = ((y * self.w + x) * 4) as usize;
        self.px[i..i + 4].copy_from_slice(&p);
    }

    /// The pixels of `r` (clamped at the edges).
    pub fn crop(&self, r: PhysRect) -> Img {
        let mut out = Img::new(r.width, r.height);
        for y in 0..r.height {
            for x in 0..r.width {
                out.set(x, y, self.get(r.x + x as i32, r.y + y as i32));
            }
        }
        out
    }
}

pub(crate) fn empty_bitmap() -> Bitmap {
    match Bitmap::new(1, 1, PixelFormat::Rgba8, vec![0u8; 4].into()) {
        Ok(b) => b,
        Err(_) => unreachable!("a 1x1 bitmap has 4 bytes"),
    }
}

/// Three box-blur passes (≈ Gaussian) with the given radius.
pub(crate) fn blur(img: &Img, radius: u32) -> Img {
    if radius == 0 || img.w == 0 || img.h == 0 {
        return img.clone();
    }
    let r = radius.div_ceil(3).max(1) as i32;
    let mut cur = img.clone();
    for _ in 0..3 {
        cur = box_pass(&cur, r, true);
        cur = box_pass(&cur, r, false);
    }
    cur
}

fn box_pass(img: &Img, r: i32, horizontal: bool) -> Img {
    let mut out = Img::new(img.w, img.h);
    let n = (2 * r + 1) as u32;
    let (outer, inner) = if horizontal { (img.h, img.w) } else { (img.w, img.h) };
    for o in 0..outer {
        let at = |i: i32| if horizontal { img.get(i, o as i32) } else { img.get(o as i32, i) };
        let mut sum = [0u32; 4];
        for i in -r..=r {
            let p = at(i);
            for c in 0..4 {
                sum[c] += p[c] as u32;
            }
        }
        for i in 0..inner as i32 {
            let p = [(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8, (sum[3] / n) as u8];
            if horizontal {
                out.set(i as u32, o, p)
            } else {
                out.set(o, i as u32, p)
            }
            let (add, sub) = (at(i + r + 1), at(i - r));
            for c in 0..4 {
                sum[c] = sum[c] + add[c] as u32 - sub[c] as u32;
            }
        }
    }
    out
}

/// Averages `size`×`size` blocks, aligned to the image origin.
pub(crate) fn pixelate(img: &Img, size: u32) -> Img {
    let size = size.max(1);
    let mut out = img.clone();
    let mut by = 0;
    while by < img.h {
        let mut bx = 0;
        while bx < img.w {
            let (bw, bh) = (size.min(img.w - bx), size.min(img.h - by));
            let mut sum = [0u64; 4];
            for y in by..by + bh {
                for x in bx..bx + bw {
                    let p = img.get(x as i32, y as i32);
                    for c in 0..4 {
                        sum[c] += p[c] as u64;
                    }
                }
            }
            let n = (bw * bh) as u64;
            let avg = [(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8, (sum[3] / n) as u8];
            for y in by..by + bh {
                for x in bx..bx + bw {
                    out.set(x, y, avg);
                }
            }
            bx += size;
        }
        by += size;
    }
    out
}

/// Multiplies by a highlighter colour.
pub(crate) fn highlight(img: &Img, color: Rgba) -> Img {
    let mut out = img.clone();
    for p in out.px.as_chunks_mut::<4>().0 {
        p[0] = (p[0] as u16 * color.r as u16 / 255) as u8;
        p[1] = (p[1] as u16 * color.g as u16 / 255) as u8;
        p[2] = (p[2] as u16 * color.b as u16 / 255) as u8;
    }
    out
}

pub(crate) fn solid(w: u32, h: u32, color: Rgba) -> Img {
    let mut out = Img::new(w, h);
    for p in out.px.as_chunks_mut::<4>().0 {
        *p = [color.r, color.g, color.b, color.a];
    }
    out
}

/// Nearest-neighbour scale to an exact size.
pub(crate) fn scale_nearest(img: &Img, w: u32, h: u32) -> Img {
    let mut out = Img::new(w, h);
    if img.w == 0 || img.h == 0 {
        return out;
    }
    for y in 0..h {
        let sy = (y as u64 * img.h as u64 / h.max(1) as u64) as i32;
        for x in 0..w {
            let sx = (x as u64 * img.w as u64 / w.max(1) as u64) as i32;
            out.set(x, y, img.get(sx, sy));
        }
    }
    out
}

pub(crate) fn rotate90(img: &Img, clockwise: bool) -> Img {
    let mut out = Img::new(img.h, img.w);
    for y in 0..img.h {
        for x in 0..img.w {
            let p = img.get(x as i32, y as i32);
            let (nx, ny) = if clockwise { (img.h - 1 - y, x) } else { (y, img.w - 1 - x) };
            out.set(nx, ny, p);
        }
    }
    out
}

pub(crate) fn flip(img: &Img, horizontal: bool) -> Img {
    let mut out = Img::new(img.w, img.h);
    for y in 0..img.h {
        for x in 0..img.w {
            let (sx, sy) = if horizontal { (img.w - 1 - x, y) } else { (x, img.h - 1 - y) };
            out.set(x, y, img.get(sx as i32, sy as i32));
        }
    }
    out
}

/// Removes columns `from..to` (vertical) or rows `from..to` and joins the rest.
pub(crate) fn cut_out(img: &Img, vertical: bool, from: u32, to: u32) -> Img {
    let len = if vertical { img.w } else { img.h };
    let (from, to) = (from.min(len), to.min(len).max(from.min(len)));
    let removed = to - from;
    if removed == 0 || removed >= len {
        return img.clone();
    }
    let (w, h) = if vertical { (img.w - removed, img.h) } else { (img.w, img.h - removed) };
    let mut out = Img::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let (sx, sy) = if vertical { (if x < from { x } else { x + removed }, y) } else { (x, if y < from { y } else { y + removed }) };
            out.set(x, y, img.get(sx as i32, sy as i32));
        }
    }
    out
}

/// Trims borders that match the top-left pixel.
pub(crate) fn auto_crop_rect(img: &Img) -> PhysRect {
    let bg = img.get(0, 0);
    let same = |x: u32, y: u32| img.get(x as i32, y as i32) == bg;
    let (mut x0, mut y0, mut x1, mut y1) = (0u32, 0u32, img.w, img.h);
    while y0 < y1 && (0..img.w).all(|x| same(x, y0)) {
        y0 += 1;
    }
    while y1 > y0 && (0..img.w).all(|x| same(x, y1 - 1)) {
        y1 -= 1;
    }
    while x0 < x1 && (y0..y1).all(|y| same(x0, y)) {
        x0 += 1;
    }
    while x1 > x0 && (y0..y1).all(|y| same(x1 - 1, y)) {
        x1 -= 1;
    }
    if x1 <= x0 || y1 <= y0 {
        return PhysRect { x: 0, y: 0, width: img.w, height: img.h };
    }
    PhysRect { x: x0 as i32, y: y0 as i32, width: x1 - x0, height: y1 - y0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern() -> Img {
        let mut i = Img::new(4, 3);
        for y in 0..3 {
            for x in 0..4 {
                i.set(x, y, [x as u8 * 10, y as u8 * 10, 0, 255]);
            }
        }
        i
    }

    #[test]
    fn rotate_four_times_is_identity() {
        let p = pattern();
        let mut r = p.clone();
        for _ in 0..4 {
            r = rotate90(&r, true);
        }
        assert_eq!(r, p);
        assert_eq!(rotate90(&rotate90(&p, true), false), p);
    }

    #[test]
    fn flip_twice_and_cut_out() {
        let p = pattern();
        assert_eq!(flip(&flip(&p, true), true), p);
        let c = cut_out(&p, true, 1, 3);
        assert_eq!((c.w, c.h), (2, 3));
        assert_eq!(c.get(1, 0), p.get(3, 0));
    }

    #[test]
    fn pixelate_one_and_blur_zero_are_identity() {
        let p = pattern();
        assert_eq!(pixelate(&p, 1), p);
        assert_eq!(blur(&p, 0), p);
    }

    #[test]
    fn auto_crop_trims_uniform_border() {
        let mut i = solid(6, 6, Rgba::WHITE);
        i.set(2, 3, [0, 0, 0, 255]);
        i.set(3, 3, [0, 0, 0, 255]);
        assert_eq!(auto_crop_rect(&i), PhysRect { x: 2, y: 3, width: 2, height: 1 });
    }
}
