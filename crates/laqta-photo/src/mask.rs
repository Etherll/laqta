//! 8-bit masks at document size, used for layer masks and selections. The morphology (grow and
//! shrink) and the blur (feathering, drop shadows) are shared with the layer styles.

use crate::PhotoError;

/// An 8-bit coverage mask, row-major, `width * height` bytes. 255 is fully selected or visible.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mask {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

impl Mask {
    /// A mask filled with one value.
    pub fn filled(width: u32, height: u32, value: u8) -> Mask {
        Mask { width, height, data: vec![value; width as usize * height as usize] }
    }

    /// A mask from raw bytes. `data` must hold exactly `width * height` bytes.
    pub fn from_vec(width: u32, height: u32, data: Vec<u8>) -> Result<Mask, PhotoError> {
        if data.len() != width as usize * height as usize {
            return Err(PhotoError::Invalid("mask data length does not match its size"));
        }
        Ok(Mask { width, height, data })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }

    /// The value at `(x, y)`, or 0 outside the mask.
    pub fn get(&self, x: i32, y: i32) -> u8 {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return 0;
        }
        self.data[y as usize * self.width as usize + x as usize]
    }

    /// The value at `(x, y)` as coverage in 0..1.
    pub(crate) fn coverage(&self, x: i32, y: i32) -> f32 {
        f32::from(self.get(x, y)) / 255.0
    }

    pub(crate) fn inverted(&self) -> Mask {
        Mask { width: self.width, height: self.height, data: self.data.iter().map(|v| 255 - v).collect() }
    }

    /// Grows the set pixels by `radius` (square structuring element).
    pub(crate) fn dilate(&self, radius: u32) -> Mask {
        self.morph(radius, true)
    }

    /// Shrinks the set pixels by `radius` (square structuring element).
    pub(crate) fn erode(&self, radius: u32) -> Mask {
        self.morph(radius, false)
    }

    fn morph(&self, radius: u32, grow: bool) -> Mask {
        if radius == 0 || self.width == 0 || self.height == 0 {
            return self.clone();
        }
        let (w, h) = (self.width as usize, self.height as usize);
        // Separable: a square window is a row pass followed by a column pass.
        let mut tmp = vec![0u8; w * h];
        for (row, out) in self.data.chunks_exact(w).zip(tmp.chunks_exact_mut(w)) {
            out.copy_from_slice(&extreme_1d(row, radius as usize, grow));
        }
        let mut out = vec![0u8; w * h];
        let mut column = vec![0u8; h];
        for x in 0..w {
            for (y, c) in column.iter_mut().enumerate() {
                *c = tmp[y * w + x];
            }
            for (y, v) in extreme_1d(&column, radius as usize, grow).into_iter().enumerate() {
                out[y * w + x] = v;
            }
        }
        Mask { width: self.width, height: self.height, data: out }
    }

    /// A separable Gaussian blur with `sigma` pixels, zero outside the mask.
    pub(crate) fn blur(&self, sigma: f32) -> Mask {
        if sigma <= 0.0 || self.width == 0 || self.height == 0 {
            return self.clone();
        }
        let (w, h) = (self.width as usize, self.height as usize);
        let radius = (3.0 * sigma).ceil() as i64;
        let mut kernel: Vec<f32> =
            (-radius..=radius).map(|i| (-((i * i) as f32) / (2.0 * sigma * sigma)).exp()).collect();
        let total: f32 = kernel.iter().sum();
        for k in &mut kernel {
            *k /= total;
        }
        let src: Vec<f32> = self.data.iter().map(|&v| f32::from(v)).collect();
        let mut tmp = vec![0.0_f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let mut acc = 0.0;
                for (k, &kv) in kernel.iter().enumerate() {
                    let sx = x as i64 + k as i64 - radius;
                    if (0..w as i64).contains(&sx) {
                        acc += kv * src[y * w + sx as usize];
                    }
                }
                tmp[y * w + x] = acc;
            }
        }
        let mut out = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..w {
                let mut acc = 0.0;
                for (k, &kv) in kernel.iter().enumerate() {
                    let sy = y as i64 + k as i64 - radius;
                    if (0..h as i64).contains(&sy) {
                        acc += kv * tmp[sy as usize * w + x];
                    }
                }
                out[y * w + x] = acc.round().clamp(0.0, 255.0) as u8;
            }
        }
        Mask { width: self.width, height: self.height, data: out }
    }

    /// Moves the mask by `(dx, dy)` into a new `width` x `height` mask. New pixels take `fill`.
    pub(crate) fn resized(&self, width: u32, height: u32, dx: i32, dy: i32, fill: u8) -> Mask {
        let mut out = Mask::filled(width, height, fill);
        for y in 0..height as i64 {
            for x in 0..width as i64 {
                let (sx, sy) = (x - i64::from(dx), y - i64::from(dy));
                if (0..self.width as i64).contains(&sx) && (0..self.height as i64).contains(&sy) {
                    out.data[y as usize * width as usize + x as usize] =
                        self.data[sy as usize * self.width as usize + sx as usize];
                }
            }
        }
        out
    }
}

/// The max (`grow`) or min over a window of `2 r + 1` samples, in O(n) (van Herk / Gil-Werman).
/// Samples outside the line are unselected (0), so an erosion starts at the mask's edge.
fn extreme_1d(src: &[u8], r: usize, grow: bool) -> Vec<u8> {
    let n = src.len();
    if r == 0 || n == 0 {
        return src.to_vec();
    }
    let f = |a: u8, b: u8| if grow { a.max(b) } else { a.min(b) };
    let w = 2 * r + 1;
    let len = n + 2 * r;
    let mut padded = vec![0u8; len];
    padded[r..r + n].copy_from_slice(src);
    // g: running extreme from each block start; h: running extreme back from each block end.
    let mut g = vec![0u8; len];
    let mut h = vec![0u8; len];
    for j in 0..len {
        g[j] = if j % w == 0 { padded[j] } else { f(g[j - 1], padded[j]) };
    }
    for j in (0..len).rev() {
        h[j] = if j % w == w - 1 || j == len - 1 { padded[j] } else { f(h[j + 1], padded[j]) };
    }
    (0..n).map(|i| f(h[i], g[i + w - 1])).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The naive window extreme, with samples outside the line counted as 0.
    fn naive(line: &[u8], r: usize, grow: bool) -> Vec<u8> {
        (0..line.len() as i64)
            .map(|i| {
                let vals = (i - r as i64..=i + r as i64).map(|j| if (0..line.len() as i64).contains(&j) { line[j as usize] } else { 0 });
                if grow { vals.max().unwrap_or(0) } else { vals.min().unwrap_or(0) }
            })
            .collect()
    }

    #[test]
    fn extreme_matches_naive_window() {
        let line = [3u8, 9, 1, 7, 4, 200, 255, 0, 15, 255, 255];
        for r in 0..5usize {
            assert_eq!(extreme_1d(&line, r, true), naive(&line, r, true), "max r={r}");
            assert_eq!(extreme_1d(&line, r, false), naive(&line, r, false), "min r={r}");
        }
    }
}
