//! Kernels for the 17 adjustments. Most are per-pixel maps or lookup tables.
//! The local ones (Clarity, Dehaze, Shadows & highlights) use a separable box blur or min filter.
//! Everything is a pure function of its inputs: rayon only splits rows, and each pixel depends on
//! its own value or a fixed neighbourhood, so the output doesn't depend on the thread count.

use laqta_domain::Bitmap;
use laqta_domain::color::Rgba;
use laqta_domain::effects::adjust::Channel;
use rayon::prelude::*;

use super::adjust::{LUMA, bitmap, map_pixels, to_u8};
use super::draw::hash_unit;

/// RGB in 0..1. Alpha is left to the caller.
fn unit(p: [u8; 4]) -> [f32; 3] {
    [f32::from(p[0]) / 255.0, f32::from(p[1]) / 255.0, f32::from(p[2]) / 255.0]
}

fn rgb_unit(c: Rgba) -> [f32; 3] {
    unit([c.r, c.g, c.b, c.a])
}

fn luma(rgb: [f32; 3]) -> f32 {
    rgb[0] * LUMA[0] + rgb[1] * LUMA[1] + rgb[2] * LUMA[2]
}

/// Packs an RGB value in 0..1 with its alpha into a pixel.
fn pack(rgb: [f32; 3], alpha: u8) -> [u8; 4] {
    [to_u8(rgb[0]), to_u8(rgb[1]), to_u8(rgb[2]), alpha]
}

/// Runs `f(index, pixel)` on every pixel, one row per task. `index` is the row-major pixel number.
fn map_indexed(image: &Bitmap, f: impl Fn(usize, [u8; 4]) -> [u8; 4] + Sync) -> Bitmap {
    let w = image.width() as usize;
    let mut px = image.pixels().to_vec();
    px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for (x, p) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            *p = f(y * w + x, *p);
        }
    });
    bitmap(image.width(), image.height(), px)
}

/// Luma of every pixel, 0..1. Transparent pixels count too; callers that care skip them by alpha.
fn luma_field(image: &Bitmap) -> Vec<f32> {
    image.pixels().as_chunks::<4>().0.iter().map(|p| luma(unit(*p))).collect()
}

fn smooth_step(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn linear_to_srgb(l: f32) -> f32 {
    if l <= 0.003_130_8 { l * 12.92 } else { 1.055 * l.powf(1.0 / 2.4) - 0.055 }
}

/// Box blur of a `w` x `h` field, radius `r`, with the edges clamped. Separable, so the cost is
/// O(w * h) whatever the radius.
fn box_blur(field: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut rows = vec![0.0f32; field.len()];
    for (src, dst) in field.chunks_exact(w).zip(rows.chunks_exact_mut(w)) {
        box_line(src, r, dst);
    }
    let mut out = vec![0.0f32; field.len()];
    let mut column = vec![0.0f32; h];
    let mut blurred = vec![0.0f32; h];
    for x in 0..w {
        for (y, v) in column.iter_mut().enumerate() {
            *v = rows[y * w + x];
        }
        box_line(&column, r, &mut blurred);
        for (y, v) in blurred.iter().enumerate() {
            out[y * w + x] = *v;
        }
    }
    out
}

/// One line of a box blur: the mean of `line[i - r ..= i + r]`, the ends clamped, from prefix sums.
fn box_line(line: &[f32], r: usize, out: &mut [f32]) {
    let n = line.len();
    let mut prefix = vec![0.0f64; n + 1];
    for (i, v) in line.iter().enumerate() {
        prefix[i + 1] = prefix[i] + f64::from(*v);
    }
    for (i, o) in out.iter_mut().enumerate().take(n) {
        let lo = i.saturating_sub(r);
        let hi = (i + r).min(n - 1);
        *o = ((prefix[hi + 1] - prefix[lo]) / (hi - lo + 1) as f64) as f32;
    }
}

/// Minimum over a (2r + 1) square of a `w` x `h` field, the edges clamped. Separable.
fn min_filter(field: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut rows = vec![0.0f32; field.len()];
    for y in 0..h {
        for x in 0..w {
            let lo = x.saturating_sub(r);
            let hi = (x + r).min(w - 1);
            rows[y * w + x] = field[y * w + lo..=y * w + hi].iter().copied().fold(f32::INFINITY, f32::min);
        }
    }
    let mut out = vec![0.0f32; field.len()];
    for y in 0..h {
        let (lo, hi) = (y.saturating_sub(r), (y + r).min(h - 1));
        for x in 0..w {
            out[y * w + x] = (lo..=hi).map(|yy| rows[yy * w + x]).fold(f32::INFINITY, f32::min);
        }
    }
    out
}

/// Stretches the luma histogram to the full range. Pixels are counted in integer bins, so the cut
/// points don't depend on the order of the pixels. Every channel uses one mapping, so hue holds.
pub(super) fn auto_contrast(image: &Bitmap, clip: f32) -> Bitmap {
    let mut hist = [0usize; 256];
    for p in image.pixels().as_chunks::<4>().0.iter().filter(|p| p[3] != 0) {
        hist[usize::from(to_u8(luma(unit(*p))))] += 1;
    }
    let total: usize = hist.iter().sum();
    if total == 0 {
        return image.clone();
    }
    let cut = (clip.clamp(0.0, 0.5) * total as f32) as usize;
    let lo = (0..256).find(|&v| hist[..=v].iter().sum::<usize>() > cut);
    let hi = (0..256).rev().find(|&v| hist[v..].iter().sum::<usize>() > cut);
    let (Some(lo), Some(hi)) = (lo, hi) else {
        return image.clone();
    };
    if hi <= lo {
        return image.clone();
    }
    let (lo, span) = (lo as f32, (hi - lo) as f32);
    let lut: [u8; 256] = std::array::from_fn(|v| to_u8((v as f32 - lo) / span));
    map_pixels(image, |p| if p[3] == 0 { p } else { [lut[usize::from(p[0])], lut[usize::from(p[1])], lut[usize::from(p[2])], p[3]] })
}

pub(super) fn channel_swap(image: &Bitmap, red: Channel, green: Channel, blue: Channel) -> Bitmap {
    let index = |c: Channel| match c {
        Channel::Red => 0,
        Channel::Green => 1,
        Channel::Blue => 2,
    };
    let order = [index(red), index(green), index(blue)];
    map_pixels(image, |p| [p[order[0]], p[order[1]], p[order[2]], p[3]])
}

/// Midtone local contrast: the detail above a box blur of `radius` pixels, weighted toward mid grey,
/// is added to luma. Colour is scaled with luma, so hue holds.
pub(super) fn clarity(image: &Bitmap, strength: f32, radius: u32) -> Bitmap {
    let (w, h) = (image.width() as usize, image.height() as usize);
    let lum = luma_field(image);
    let blurred = box_blur(&lum, w, h, radius as usize);
    let amount = strength.clamp(0.0, 1.0) * 2.0;
    map_indexed(image, |i, p| {
        let l = lum[i];
        if l <= 1e-4 {
            return p;
        }
        let midtone = 4.0 * l * (1.0 - l);
        let target = (l + amount * midtone * (l - blurred[i])).clamp(0.0, 1.0);
        let k = target / l;
        let rgb = unit(p);
        pack(std::array::from_fn(|c| rgb[c] * k), p[3])
    })
}

/// Offsets per channel for shadows, midtones and highlights. The three weights add to 1 at every
/// luma: (1 - l)^2, 2l(1 - l) and l^2.
pub(super) fn color_balance(image: &Bitmap, shadows: [f32; 3], midtones: [f32; 3], highlights: [f32; 3]) -> Bitmap {
    map_pixels(image, |p| {
        let rgb = unit(p);
        let l = luma(rgb).clamp(0.0, 1.0);
        let (ws, wm, wh) = ((1.0 - l) * (1.0 - l), 2.0 * l * (1.0 - l), l * l);
        let out: [f32; 3] = std::array::from_fn(|c| rgb[c] + ws * shadows[c] + wm * midtones[c] + wh * highlights[c]);
        pack(out, p[3])
    })
}

/// A cross-processed look: each channel has its own gamma and gain before a smoothstep S-curve.
/// Red lifts the mids and blue drops the highlights, which gives the colour cast.
pub(super) fn cross_process(image: &Bitmap, strength: f32) -> Bitmap {
    const CURVES: [(f32, f32); 3] = [(0.9, 1.05), (1.0, 1.0), (1.15, 0.9)];
    let mix = strength.clamp(0.0, 1.0);
    let lut: [[u8; 256]; 3] = std::array::from_fn(|c| {
        let (gamma, gain) = CURVES[c];
        std::array::from_fn(|v| {
            let x = v as f32 / 255.0;
            let s = x.powf(gamma);
            let curve = (gain * s * s * (3.0 - 2.0 * s)).clamp(0.0, 1.0);
            to_u8(x + (curve - x) * mix)
        })
    });
    map_pixels(image, |p| [lut[0][usize::from(p[0])], lut[1][usize::from(p[1])], lut[2][usize::from(p[2])], p[3]])
}

/// Haze removal with the dark channel prior. The airlight is the brightest colour among the haziest
/// 0.1% of pixels, and each pixel's transmission comes from its dark channel over a `radius` patch.
pub(super) fn dehaze(image: &Bitmap, strength: f32, radius: u32) -> Bitmap {
    let (w, h) = (image.width() as usize, image.height() as usize);
    let px = image.pixels().as_chunks::<4>().0;
    let r = radius as usize;
    // The dark channel: the darkest colour of each pixel. Transparent pixels count as white.
    let dark: Vec<f32> = px.iter().map(|p| if p[3] == 0 { 1.0 } else { unit(*p).into_iter().fold(1.0, f32::min) }).collect();
    let dark_min = min_filter(&dark, w, h, r);

    let bin = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as usize;
    let mut hist = [0usize; 256];
    for (p, d) in px.iter().zip(&dark_min) {
        if p[3] != 0 {
            hist[bin(*d)] += 1;
        }
    }
    let total: usize = hist.iter().sum();
    let wanted = (total / 1000).max(1);
    let mut seen = 0;
    let mut cut = 0;
    for v in (0..256).rev() {
        seen += hist[v];
        if seen >= wanted {
            cut = v;
            break;
        }
    }
    let mut best: Option<(f32, [f32; 3])> = None;
    for (p, d) in px.iter().zip(&dark_min) {
        if p[3] == 0 || bin(*d) < cut {
            continue;
        }
        let rgb = unit(*p);
        let sum = rgb[0] + rgb[1] + rgb[2];
        if best.is_none_or(|(s, _)| sum > s) {
            best = Some((sum, rgb));
        }
    }
    let Some((_, air)) = best else {
        return image.clone();
    };
    let air = air.map(|c| c.max(0.05));

    let norm: Vec<f32> = px
        .iter()
        .map(|p| {
            if p[3] == 0 {
                return 1.0;
            }
            let rgb = unit(*p);
            (0..3).map(|c| rgb[c] / air[c]).fold(f32::INFINITY, f32::min)
        })
        .collect();
    let trans_min = min_filter(&norm, w, h, r);
    let omega = strength.clamp(0.0, 1.0);
    map_indexed(image, |i, p| {
        if p[3] == 0 {
            return p;
        }
        let t = (1.0 - omega * trans_min[i]).max(0.1);
        let rgb = unit(p);
        pack(std::array::from_fn(|c| (rgb[c] - air[c]) / t + air[c]), p[3])
    })
}

/// Luma mapped onto a gradient from `shadow` to `highlight`, blended over the image by `strength`.
pub(super) fn duotone(image: &Bitmap, shadow: Rgba, highlight: Rgba, strength: f32) -> Bitmap {
    let (dark, light) = (rgb_unit(shadow), rgb_unit(highlight));
    let mix = strength.clamp(0.0, 1.0);
    map_pixels(image, |p| {
        let rgb = unit(p);
        let l = luma(rgb).clamp(0.0, 1.0);
        let duo: [f32; 3] = std::array::from_fn(|c| dark[c] + (light[c] - dark[c]) * l);
        pack(std::array::from_fn(|c| rgb[c] + (duo[c] - rgb[c]) * mix), p[3])
    })
}

/// Multiplies linear light by 2^stops. The table goes through sRGB, so it's exact per level.
pub(super) fn exposure(image: &Bitmap, stops: f32) -> Bitmap {
    let gain = 2f32.powf(stops.clamp(-5.0, 5.0));
    let lut: [u8; 256] = std::array::from_fn(|v| {
        let linear = (srgb_to_linear(v as f32 / 255.0) * gain).clamp(0.0, 1.0);
        to_u8(linear_to_srgb(linear))
    });
    map_pixels(image, |p| [lut[usize::from(p[0])], lut[usize::from(p[1])], lut[usize::from(p[2])], p[3]])
}

/// A film stock look: blacks lifted to about 5%, a soft top end, a warm-highlight and cool-shadow
/// cast, and grain. The grain is hashed from the pixel position and `seed`, so it's the same every run.
pub(super) fn film(image: &Bitmap, strength: f32, grain: f32, seed: u64) -> Bitmap {
    let w = image.width() as usize;
    let mix = strength.clamp(0.0, 1.0);
    let grain = grain.clamp(0.0, 1.0) * 0.1;
    map_indexed(image, |i, p| {
        if p[3] == 0 {
            return p;
        }
        let rgb = unit(p);
        let l = luma(rgb).clamp(0.0, 1.0);
        let cast = [0.02 * l - 0.01 * (1.0 - l), 0.0, 0.02 * (1.0 - l) - 0.03 * l];
        let noise = (hash_unit(seed, (i % w) as u64, (i / w) as u64) * 2.0 - 1.0) as f32 * grain;
        let film: [f32; 3] = std::array::from_fn(|c| 0.05 + 0.92 * rgb[c] + cast[c] + noise);
        pack(std::array::from_fn(|c| rgb[c] + (film[c] - rgb[c]) * mix), p[3])
    })
}

/// Input black and white, gamma, and output black and white, on 0..255 levels. A white at or below
/// black is widened to one level, so the table never divides by zero.
pub(super) fn levels(image: &Bitmap, in_black: u8, in_white: u8, gamma: f32, out_black: u8, out_white: u8) -> Bitmap {
    let lo = f32::from(in_black);
    let span = (f32::from(in_white) - lo).max(1.0);
    let inv_gamma = 1.0 / gamma.clamp(0.1, 10.0);
    let (ob, ow) = (f32::from(out_black), f32::from(out_white));
    let lut: [u8; 256] = std::array::from_fn(|v| {
        let t = ((v as f32 - lo) / span).clamp(0.0, 1.0).powf(inv_gamma);
        to_u8((ob + (ow - ob) * t) / 255.0)
    });
    map_pixels(image, |p| [lut[usize::from(p[0])], lut[usize::from(p[1])], lut[usize::from(p[2])], p[3]])
}

/// Saturation up, an S-curve, and a dark vignette toward the corners.
pub(super) fn lomo(image: &Bitmap, strength: f32) -> Bitmap {
    let (w, h) = (image.width() as usize, image.height() as usize);
    let s = strength.clamp(0.0, 1.0);
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let max_r = cx.hypot(cy).max(1.0);
    map_indexed(image, |i, p| {
        if p[3] == 0 {
            return p;
        }
        let rgb = unit(p);
        let l = luma(rgb);
        let mut c: [f32; 3] = std::array::from_fn(|k| l + (rgb[k] - l) * (1.0 + s));
        for v in &mut c {
            let x = v.clamp(0.0, 1.0);
            *v = x + (x * x * (3.0 - 2.0 * x) - x) * 0.6 * s;
        }
        let x = (i % w) as f32 + 0.5 - cx;
        let y = (i / w) as f32 + 0.5 - cy;
        let vignette = 1.0 - 0.7 * s * smooth_step(0.35, 1.0, x.hypot(y) / max_r);
        pack(c.map(|v| v * vignette), p[3])
    })
}

/// Lifts shadows and pulls highlights down, judged against the box-blurred luma within `radius`,
/// so a dark pixel in a bright area is treated as a shadow only where it is one.
pub(super) fn shadows_highlights(image: &Bitmap, shadows: f32, highlights: f32, radius: u32) -> Bitmap {
    let (w, h) = (image.width() as usize, image.height() as usize);
    let local = box_blur(&luma_field(image), w, h, radius as usize);
    let (lift, pull) = (shadows.clamp(0.0, 1.0), highlights.clamp(0.0, 1.0));
    map_indexed(image, |i, p| {
        if p[3] == 0 {
            return p;
        }
        let m = local[i].clamp(0.0, 1.0);
        let up = lift * (1.0 - m) * (1.0 - m);
        let down = pull * m * m;
        let rgb = unit(p);
        pack(std::array::from_fn(|c| rgb[c] + up * (1.0 - rgb[c]) - down * rgb[c]), p[3])
    })
}

/// Channel values above `threshold` are inverted, so the image turns partly into a negative.
pub(super) fn solarize(image: &Bitmap, threshold: u8) -> Bitmap {
    let t = usize::from(threshold);
    let lut: [u8; 256] = std::array::from_fn(|v| if v > t { 255 - v as u8 } else { v as u8 });
    map_pixels(image, |p| [lut[usize::from(p[0])], lut[usize::from(p[1])], lut[usize::from(p[2])], p[3]])
}

/// Shadows move toward `shadow`, highlights toward `highlight`, each by `strength` at the far end.
/// `balance` shifts the luma the split is judged on.
pub(super) fn split_toning(image: &Bitmap, shadow: Rgba, highlight: Rgba, strength: f32, balance: f32) -> Bitmap {
    let (dark, light) = (rgb_unit(shadow), rgb_unit(highlight));
    let k = strength.clamp(0.0, 1.0) * 0.5;
    let shift = balance.clamp(-1.0, 1.0) * 0.25;
    map_pixels(image, |p| {
        let rgb = unit(p);
        let l = (luma(rgb) + shift).clamp(0.0, 1.0);
        let (ws, wh) = ((1.0 - l) * (1.0 - l) * k, l * l * k);
        let out: [f32; 3] = std::array::from_fn(|c| {
            let toned = rgb[c] + (dark[c] - rgb[c]) * ws;
            toned + (light[c] - toned) * wh
        });
        pack(out, p[3])
    })
}

/// Per-channel gains: red and blue move against each other for temperature, green moves for tint.
pub(super) fn temperature_tint(image: &Bitmap, temperature: f32, tint: f32) -> Bitmap {
    let t = temperature.clamp(-1.0, 1.0);
    let g = tint.clamp(-1.0, 1.0);
    let gains = [1.0 + 0.25 * t, 1.0 + 0.2 * g, 1.0 - 0.25 * t];
    map_pixels(image, |p| {
        let rgb = unit(p);
        pack(std::array::from_fn(|c| rgb[c] * gains[c]), p[3])
    })
}

/// Black and white by luma: at or above `level` is white, below is black.
pub(super) fn threshold(image: &Bitmap, level: u8) -> Bitmap {
    let level = f32::from(level);
    map_pixels(image, |p| {
        let v = if luma(unit(p)) * 255.0 >= level { 255 } else { 0 };
        [v, v, v, p[3]]
    })
}

/// Pushes each colour away from its luma. The push is largest for the least saturated pixels, so
/// muted colours gain most and already vivid ones barely change.
pub(super) fn vibrance(image: &Bitmap, value: f32) -> Bitmap {
    let v = value.clamp(0.0, 1.0);
    map_pixels(image, |p| {
        let rgb = unit(p);
        let mx = rgb.iter().copied().fold(0.0, f32::max);
        let mn = rgb.iter().copied().fold(1.0, f32::min);
        let sat = if mx > 0.0 { (mx - mn) / mx } else { 0.0 };
        let boost = 1.0 + v * (1.0 - sat);
        let l = luma(rgb);
        pack(std::array::from_fn(|c| l + (rgb[c] - l) * boost), p[3])
    })
}
