//! Blurs: box (three passes), Gaussian, motion and zoom. They average premultiplied colour, so an
//! edge between opaque and transparent pixels doesn't darken.

use rayon::prelude::*;

use super::{Buf, premultiply, unit, unpremultiply, window_index};

/// Box blur of radius `radius` in three passes, which is close to a Gaussian. Radius 0 is identity.
pub(super) fn box_blur(src: &Buf, radius: u32) -> Buf {
    if radius == 0 {
        return src.clone();
    }
    let mut pm = premultiply(&src.px);
    box3(&mut pm, src.w, src.h, 4, radius as usize);
    Buf { w: src.w, h: src.h, px: unpremultiply(&pm) }
}

/// Three box passes of radius `r` over `ch` interleaved channels, with clamped edges. The shadow and
/// glow masks use it on one channel.
pub(super) fn box3(px: &mut [f32], w: usize, h: usize, ch: usize, r: usize) {
    if r == 0 {
        return;
    }
    let mut tmp = vec![0f32; px.len()];
    for _ in 0..3 {
        box_rows(px, &mut tmp, w, ch, r);
        box_columns(&tmp, px, w, h, ch, r);
    }
}

/// Gaussian blur with standard deviation `sigma` in pixels. A sigma below 0.001, or NaN, is identity.
pub(super) fn gaussian(src: &Buf, sigma: f32) -> Buf {
    if sigma.is_nan() || sigma < 0.001 {
        return src.clone();
    }
    let kernel = gaussian_kernel(sigma);
    let (w, h) = (src.w, src.h);
    let pm = premultiply(&src.px);
    let mut across = vec![0f32; pm.len()];
    let mut out = vec![0f32; pm.len()];
    convolve_rows(&pm, &mut across, w, 4, &kernel);
    convolve_columns(&across, &mut out, w, h, 4, &kernel);
    Buf { w, h, px: unpremultiply(&out) }
}

/// Average along a line of `distance` pixels, centred on each pixel, at `angle` degrees. Samples are
/// bilinear. Distance 0 is identity.
pub(super) fn motion(src: &Buf, distance: u32, angle: f32) -> Buf {
    if distance == 0 {
        return src.clone();
    }
    let angle = if angle.is_finite() { angle } else { 0.0 };
    let (sin, cos) = angle.to_radians().sin_cos();
    let steps = distance as usize + 1;
    let half = distance as f32 / 2.0;
    let (w, h) = (src.w, src.h);
    let pm = premultiply(&src.px);
    let mut out = vec![0f32; pm.len()];
    out.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let mut acc = [0f32; 4];
            for i in 0..steps {
                let t = i as f32 - half;
                let s = sample(&pm, w, h, x as f32 + t * cos, y as f32 + t * sin);
                for (a, v) in acc.iter_mut().zip(s) {
                    *a += v;
                }
            }
            let inv = 1.0 / steps as f32;
            for (d, a) in row[x * 4..x * 4 + 4].iter_mut().zip(acc) {
                *d = a * inv;
            }
        }
    });
    Buf { w, h, px: unpremultiply(&out) }
}

/// Blur toward a centre given as fractions of the image size. Each pixel averages 16 bilinear samples
/// along the way from itself to the centre; `amount` 0..=1 is how far along that way they reach.
/// An amount of 0 (or NaN) is identity.
pub(super) fn zoom(src: &Buf, amount: f32, center_x: f32, center_y: f32) -> Buf {
    const SAMPLES: usize = 16;
    if amount.is_nan() || amount <= 0.0 {
        return src.clone();
    }
    let amount = unit(amount);
    let (w, h) = (src.w, src.h);
    let center = |c: f32, len: usize| if c.is_finite() { c * len as f32 } else { len as f32 / 2.0 };
    let (cx, cy) = (center(center_x, w), center(center_y, h));
    let pm = premultiply(&src.px);
    let mut out = vec![0f32; pm.len()];
    out.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let (px, py) = (x as f32, y as f32);
            let mut acc = [0f32; 4];
            for i in 0..SAMPLES {
                let s = amount * i as f32 / (SAMPLES - 1) as f32;
                let v = sample(&pm, w, h, px + (cx - px) * s, py + (cy - py) * s);
                for (a, c) in acc.iter_mut().zip(v) {
                    *a += c;
                }
            }
            for (d, a) in row[x * 4..x * 4 + 4].iter_mut().zip(acc) {
                *d = a / SAMPLES as f32;
            }
        }
    });
    Buf { w, h, px: unpremultiply(&out) }
}

/// Bilinear sample of premultiplied pixels at (x, y), clamped to the image. `w` and `h` are at least 1.
fn sample(pm: &[f32], w: usize, h: usize, x: f32, y: f32) -> [f32; 4] {
    let x = x.clamp(0.0, (w - 1) as f32);
    let y = y.clamp(0.0, (h - 1) as f32);
    let (x0f, y0f) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0f, y - y0f);
    let (x0, y0) = (x0f as usize, y0f as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let at = |xx: usize, yy: usize| {
        let i = (yy * w + xx) * 4;
        [pm[i], pm[i + 1], pm[i + 2], pm[i + 3]]
    };
    let (p00, p10, p01, p11) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
    std::array::from_fn(|c| {
        let top = p00[c] + (p10[c] - p00[c]) * fx;
        let bottom = p01[c] + (p11[c] - p01[c]) * fx;
        top + (bottom - top) * fy
    })
}

/// Normalised Gaussian weights, `2 * ceil(3 * sigma) + 1` long.
fn gaussian_kernel(sigma: f32) -> Vec<f32> {
    let radius = (3.0 * sigma).ceil() as usize;
    let denom = 2.0 * sigma * sigma;
    let mut kernel: Vec<f32> = (0..=2 * radius)
        .map(|i| {
            let d = i as f32 - radius as f32;
            (-(d * d) / denom).exp()
        })
        .collect();
    let total: f32 = kernel.iter().sum();
    for k in &mut kernel {
        *k /= total;
    }
    kernel
}

/// Horizontal pass of a separable kernel, one row per task.
fn convolve_rows(src: &[f32], dst: &mut [f32], w: usize, ch: usize, kernel: &[f32]) {
    let row = w * ch;
    let radius = kernel.len() / 2;
    dst.par_chunks_mut(row).zip(src.par_chunks(row)).for_each(|(d, s)| {
        for x in 0..w {
            for c in 0..ch {
                d[x * ch + c] = kernel.iter().enumerate().map(|(j, wt)| wt * s[window_index(x, j, radius, w) * ch + c]).sum();
            }
        }
    });
}

/// Vertical pass of a separable kernel, one output row per task.
fn convolve_columns(src: &[f32], dst: &mut [f32], w: usize, h: usize, ch: usize, kernel: &[f32]) {
    let row = w * ch;
    let radius = kernel.len() / 2;
    dst.par_chunks_mut(row).enumerate().for_each(|(y, d)| {
        d.fill(0.0);
        for (j, &wt) in kernel.iter().enumerate() {
            let sy = window_index(y, j, radius, h);
            for (dv, sv) in d.iter_mut().zip(&src[sy * row..(sy + 1) * row]) {
                *dv += wt * sv;
            }
        }
    });
}

/// One box pass, horizontal: each row is a running sum, so the cost doesn't depend on the radius.
fn box_rows(src: &[f32], dst: &mut [f32], w: usize, ch: usize, r: usize) {
    let row = w * ch;
    dst.par_chunks_mut(row).zip(src.par_chunks(row)).for_each(|(d, s)| box_line(s, d, w, ch, r));
}

fn box_line(src: &[f32], dst: &mut [f32], w: usize, ch: usize, r: usize) {
    let (r, last) = (r as isize, w as isize - 1);
    let at = |v: isize| v.clamp(0, last) as usize * ch;
    let norm = 1.0 / (2 * r + 1) as f64;
    let mut acc = vec![0f64; ch];
    for k in -r..=r {
        add_scaled(&mut acc, &src[at(k)..][..ch], 1.0);
    }
    for x in 0..w {
        for (d, a) in dst[x * ch..(x + 1) * ch].iter_mut().zip(&acc) {
            *d = (*a * norm) as f32;
        }
        let xi = x as isize;
        add_scaled(&mut acc, &src[at(xi + r + 1)..][..ch], 1.0);
        add_scaled(&mut acc, &src[at(xi - r)..][..ch], -1.0);
    }
}

/// One box pass, vertical: a running sum of whole rows, updated row by row.
fn box_columns(src: &[f32], dst: &mut [f32], w: usize, h: usize, ch: usize, r: usize) {
    let row = w * ch;
    let (r, last) = (r as isize, h as isize - 1);
    let at = |v: isize| v.clamp(0, last) as usize * row;
    let norm = 1.0 / (2 * r + 1) as f64;
    let mut acc = vec![0f64; row];
    for k in -r..=r {
        add_scaled(&mut acc, &src[at(k)..][..row], 1.0);
    }
    for y in 0..h {
        for (d, a) in dst[y * row..(y + 1) * row].iter_mut().zip(&acc) {
            *d = (*a * norm) as f32;
        }
        let yi = y as isize;
        add_scaled(&mut acc, &src[at(yi + r + 1)..][..row], 1.0);
        add_scaled(&mut acc, &src[at(yi - r)..][..row], -1.0);
    }
}

fn add_scaled(acc: &mut [f64], px: &[f32], scale: f64) {
    for (a, p) in acc.iter_mut().zip(px) {
        *a += scale * f64::from(*p);
    }
}
