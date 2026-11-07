//! Per-pixel tone effects: colour depth, posterize, dither, noise and vignette. Alpha is kept.

use laqta_domain::color::Rgba;
use rayon::prelude::*;

use super::{Buf, hash, to_u8, unit};

/// Keeps `bits` bits per channel, 1..=8. Eight bits is identity.
pub(super) fn color_depth(src: &Buf, bits: u8) -> Buf {
    let bits = u32::from(bits.clamp(1, 8));
    let shift = 8 - bits;
    let max = (1u32 << bits) - 1;
    map_channels(src, |v| ((u32::from(v) >> shift) * 255 / max) as u8)
}

/// Quantises each channel to `levels` evenly spaced values, 2..=255.
pub(super) fn posterize(src: &Buf, levels: u8) -> Buf {
    let levels = u32::from(levels.clamp(2, 255));
    map_channels(src, |v| posterize_value(v, levels))
}

/// The bin of `v` among `levels` values (at least 2), mapped back to 0..=255. With 256 levels this
/// is identity.
fn posterize_value(v: u8, levels: u32) -> u8 {
    debug_assert!(levels >= 2);
    let q = (u32::from(v) * levels / 256).min(levels - 1);
    (q * 255 / (levels - 1)) as u8
}

/// Floyd–Steinberg error diffusion per channel, to `levels` values, 2..=255. Rows depend on the
/// rows before them, so this runs sequentially.
pub(super) fn dither(src: &Buf, levels: u8) -> Buf {
    let levels = u32::from(levels.clamp(2, 255));
    let (w, h) = (src.w, src.h);
    let mut out = src.clone();
    for c in 0..3 {
        let mut work: Vec<f32> = (0..w * h).map(|i| f32::from(src.px[i * 4 + c])).collect();
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                let old = work[i].clamp(0.0, 255.0);
                let q = nearest_level(old, levels);
                let err = old - f32::from(q);
                out.px[i * 4 + c] = q;
                if x + 1 < w {
                    work[i + 1] += err * 7.0 / 16.0;
                }
                if y + 1 < h {
                    if x > 0 {
                        work[i + w - 1] += err * 3.0 / 16.0;
                    }
                    work[i + w] += err * 5.0 / 16.0;
                    if x + 1 < w {
                        work[i + w + 1] += err / 16.0;
                    }
                }
            }
        }
    }
    out
}

/// The level nearest to `v` (0..=255) among `levels` evenly spaced values, as in `posterize_value`.
fn nearest_level(v: f32, levels: u32) -> u8 {
    let q = (v * (levels - 1) as f32 / 255.0).round().clamp(0.0, (levels - 1) as f32) as u32;
    (q * 255 / (levels - 1)) as u8
}

/// Adds uniform noise in `-amount..=amount` to each colour channel. Each value is hashed from the seed
/// and the byte index, so the result doesn't depend on which thread computes it.
pub(super) fn noise(src: &Buf, amount: u8, seed: u64) -> Buf {
    if amount == 0 {
        return src.clone();
    }
    let amount = i64::from(amount);
    let span = (2 * amount + 1) as u64;
    let w = src.w;
    let mut out = src.clone();
    out.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            for c in 0..3 {
                let index = ((y * w + x) * 4 + c) as u64;
                let delta = (hash(seed, index) % span) as i64 - amount;
                let v = i64::from(row[x * 4 + c]) + delta;
                row[x * 4 + c] = v.clamp(0, 255) as u8;
            }
        }
    });
    out
}

/// Blends each colour channel toward `color` by a factor that grows with distance from the centre.
/// `radius` (0..=1) is the fraction of the half-diagonal where the falloff starts, and `strength`
/// (0..=1) how far it goes at the corners. The falloff is quadratic.
pub(super) fn vignette(src: &Buf, strength: f32, radius: f32, color: Rgba) -> Buf {
    let strength = unit(strength);
    let radius = unit(radius);
    let (w, h) = (src.w, src.h);
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let half_diagonal = (cx * cx + cy * cy).sqrt().max(f32::MIN_POSITIVE);
    let span = (1.0 - radius).max(1e-6);
    let target = [color.r, color.g, color.b].map(f32::from);
    let mut out = src.clone();
    out.px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let t = ((dx * dx + dy * dy).sqrt() / half_diagonal).min(1.0);
            let m = ((t - radius) / span).clamp(0.0, 1.0).powi(2) * strength;
            for (v, target) in row[x * 4..x * 4 + 3].iter_mut().zip(target) {
                let s = f32::from(*v);
                *v = to_u8(s + (target - s) * m);
            }
        }
    });
    out
}

/// Applies `f` to red, green and blue through a lookup table; alpha is kept.
fn map_channels(src: &Buf, f: impl Fn(u8) -> u8) -> Buf {
    let lut: [u8; 256] = std::array::from_fn(|v| f(v as u8));
    let mut out = src.clone();
    out.px.par_chunks_mut(src.w * 4).for_each(|row| {
        for pixel in row.as_chunks_mut::<4>().0 {
            for c in &mut pixel[..3] {
                *c = lut[usize::from(*c)];
            }
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posterize_with_256_levels_is_identity() {
        for v in 0..=255u8 {
            assert_eq!(posterize_value(v, 256), v);
        }
    }

    #[test]
    fn posterize_keeps_only_the_level_values() {
        let allowed: Vec<u8> = (0u32..4).map(|q| (q * 255 / 3) as u8).collect();
        for v in 0..=255u8 {
            assert!(allowed.contains(&posterize_value(v, 4)), "{v}");
        }
    }
}
