//! The engine's working pixels: straight-alpha RGBA8, converted from and to `Bitmap` at the edges.

use laqta_domain::{Bitmap, PixelFormat};

use crate::blend::{self, BlendMode};

/// A straight-alpha RGBA8 buffer, row-major, four bytes per pixel.
#[derive(Clone, Debug)]
pub(crate) struct Surface {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

impl Surface {
    /// A fully transparent surface.
    pub fn new(w: u32, h: u32) -> Surface {
        Surface { w, h, px: vec![0; w as usize * h as usize * 4] }
    }

    /// Copies a bitmap in, swapping channels for BGRA input.
    pub fn from_bitmap(b: &Bitmap) -> Surface {
        let mut px = b.pixels().to_vec();
        if b.format() == PixelFormat::Bgra8 {
            for c in px.as_chunks_mut::<4>().0.iter_mut() {
                c.swap(0, 2);
            }
        }
        Surface { w: b.width(), h: b.height(), px }
    }

    pub fn to_bitmap(&self) -> Bitmap {
        self.clone().into_bitmap()
    }

    pub fn into_bitmap(self) -> Bitmap {
        // The buffer is `w * h * 4` by construction, so the length check cannot fail.
        Bitmap::new(self.w, self.h, PixelFormat::Rgba8, self.px.into()).expect("surface length matches its size")
    }

    /// Index of the first byte of pixel `(x, y)`, or `None` outside the surface.
    pub fn index(&self, x: i64, y: i64) -> Option<usize> {
        if x < 0 || y < 0 || x >= i64::from(self.w) || y >= i64::from(self.h) {
            return None;
        }
        Some((y as usize * self.w as usize + x as usize) * 4)
    }

    pub fn pixel(&self, x: i64, y: i64) -> Option<[u8; 4]> {
        let i = self.index(x, y)?;
        Some([self.px[i], self.px[i + 1], self.px[i + 2], self.px[i + 3]])
    }
}

/// A rectangle of pixels in layer-local coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl PixelRect {
    /// The bytes of this rectangle in `s`, row by row.
    pub fn extract(&self, s: &Surface) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.w as usize * self.h as usize * 4);
        for row in 0..self.h {
            let start = ((self.y + row) as usize * s.w as usize + self.x as usize) * 4;
            out.extend_from_slice(&s.px[start..start + self.w as usize * 4]);
        }
        out
    }

    /// Writes `data` (as made by [`PixelRect::extract`]) back into `s`.
    pub fn write(&self, s: &mut Surface, data: &[u8]) {
        let row_bytes = self.w as usize * 4;
        for (row, src) in data.chunks_exact(row_bytes).enumerate() {
            let start = ((self.y as usize + row) * s.w as usize + self.x as usize) * 4;
            s.px[start..start + row_bytes].copy_from_slice(src);
        }
    }
}

/// Straight RGBA as 0..1 floats.
pub(crate) fn to_f32(p: &[u8]) -> [f32; 4] {
    [f32::from(p[0]) / 255.0, f32::from(p[1]) / 255.0, f32::from(p[2]) / 255.0, f32::from(p[3]) / 255.0]
}

pub(crate) fn to_u8(c: [f32; 4], p: &mut [u8]) {
    for (dst, v) in p.iter_mut().zip(c) {
        *dst = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    }
}

/// Source-over of straight colour `rgb` (0..255 per channel) with total source alpha `alpha`
/// (0..1) onto pixel `dst`.
pub(crate) fn over(dst: &mut [u8], rgb: [f32; 3], alpha: f32) {
    if alpha <= 0.0 {
        return;
    }
    let cb = to_f32(dst);
    let cs = [rgb[0] / 255.0, rgb[1] / 255.0, rgb[2] / 255.0, alpha.min(1.0)];
    to_u8(blend::composite(BlendMode::Normal, cb, cs), dst);
}

/// Whether two RGBA values differ by at most `tol` in every channel.
pub(crate) fn within(a: [u8; 4], b: [u8; 4], tol: u8) -> bool {
    a.iter().zip(b).all(|(x, y)| (i32::from(*x) - i32::from(y)).abs() <= i32::from(tol))
}
