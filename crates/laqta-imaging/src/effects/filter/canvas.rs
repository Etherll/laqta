//! Effects that change the canvas: glow, shadow, outline and reflection.
//!
//! Glow and shadow composite a blurred copy of the image's alpha under the image. Outline draws a
//! ring around the opaque shape. Reflection adds a flipped copy below. Each sizes its canvas from
//! the parameters, and a canvas past `MAX_CANVAS_PIXELS` is refused as `Invalid`.

use laqta_domain::color::Rgba;
use rayon::prelude::*;

use super::blur::box3;
use super::{Buf, EffectError, premultiplied, to_u8, unit, unpremultiply};

/// Largest canvas or mask an effect may allocate, in pixels (about 1 GB of RGBA floats at most).
const MAX_CANVAS_PIXELS: u64 = 1 << 28;

/// Glow: the image's blurred alpha, scaled by `strength` (clipped to 1) and coloured `color`, under
/// the image. The halo reaches `3 * size` past the image before the offset, and the canvas grows to
/// hold it.
pub(super) fn glow(src: &Buf, size: u32, strength: f32, color: Rgba, offset: (i32, i32)) -> Result<Buf, EffectError> {
    let radius = size as usize;
    let reach = radius.saturating_mul(3);
    let cv = Canvas::for_effect(src, reach, offset, true)?;
    let strength = if strength.is_finite() { strength.max(0.0) } else { 0.0 };
    let scale = alpha_of(color);
    let layer = drop_layer(src, &cv, reach, radius, offset, rgb_of(color, 1.0), |m| (m * strength).min(1.0) * scale)?;
    Ok(over_image(layer, &cv, src))
}

/// Shadow: the image's blurred alpha, scaled by `opacity` and coloured `color` mixed toward black by
/// `darkness`, under the image. The shadow reaches `3 * size` past the image before the offset. With
/// `auto_resize` the canvas is the union of the image and the shadow, and otherwise it stays the image's
/// size and the shadow is clipped.
pub(super) fn shadow(
    src: &Buf,
    opacity: f32,
    size: u32,
    darkness: f32,
    color: Rgba,
    offset: (i32, i32),
    auto_resize: bool,
) -> Result<Buf, EffectError> {
    let radius = size as usize;
    let reach = radius.saturating_mul(3);
    let cv = Canvas::for_effect(src, reach, offset, auto_resize)?;
    let rgb = rgb_of(color, 1.0 - unit(darkness));
    let scale = unit(opacity) * alpha_of(color);
    let layer = drop_layer(src, &cv, reach, radius, offset, rgb, |m| m * scale)?;
    Ok(over_image(layer, &cv, src))
}

/// Outline: a ring `size` pixels wide around the opaque shape (alpha of at least half), in `color`.
/// The canvas is padded by `size + padding` on every side. With `outline_only` the image is dropped
/// and only the ring remains; otherwise the image is drawn over it.
pub(super) fn outline(src: &Buf, size: u32, padding: u32, color: Rgba, outline_only: bool) -> Result<Buf, EffectError> {
    let margin = i64::from(size) + i64::from(padding);
    let (w, h) = canvas_size(src.w as i64 + 2 * margin, src.h as i64 + 2 * margin)?;
    let cv = Canvas { w, h, x0: margin as usize, y0: margin as usize };
    let mut shape = vec![false; w * h];
    for y in 0..src.h {
        for x in 0..src.w {
            shape[(y + cv.y0) * w + x + cv.x0] = src.px[(y * src.w + x) * 4 + 3] >= 128;
        }
    }
    let grown = dilate(&shape, w, h, size as usize);
    let rgb = rgb_of(color, 1.0);
    let scale = alpha_of(color);
    let mut layer = vec![0f32; w * h * 4];
    layer.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let i = y * w + x;
            if grown[i] && !shape[i] {
                row[x * 4..x * 4 + 4].copy_from_slice(&[rgb[0] * scale, rgb[1] * scale, rgb[2] * scale, scale]);
            }
        }
    });
    if outline_only {
        return Ok(Buf { w, h, px: unpremultiply(&layer) });
    }
    Ok(over_image(layer, &cv, src))
}

/// Reflection: a flipped copy of the bottom `percentage` (clamped to 100) of the image, placed under it
/// after `offset` transparent rows. Its alpha fades from `max_alpha` at the image's edge to
/// `min_alpha` at the far end. The height is `h + offset + round(h * percentage / 100)`. With `skew`
/// the copy leans right, so its far end is `skew_size` pixels along, and the width grows by
/// `skew_size`. A percentage of 0 is identity.
pub(super) fn reflection(
    src: &Buf,
    percentage: u8,
    max_alpha: u8,
    min_alpha: u8,
    offset: u32,
    skew: bool,
    skew_size: u32,
) -> Result<Buf, EffectError> {
    let (w, h) = (src.w, src.h);
    let rows = (h as u64 * u64::from(percentage.min(100)) + 50) as usize / 100;
    if rows == 0 {
        return Ok(src.clone());
    }
    let gap = offset as usize;
    let lean = if skew { skew_size as usize } else { 0 };
    let (ow, oh) = canvas_size((w + lean) as i64, (h + gap + rows) as i64)?;
    let mut px = vec![0u8; ow * oh * 4];
    let (max_a, min_a) = (f32::from(max_alpha), f32::from(min_alpha));
    px.par_chunks_mut(ow * 4).enumerate().for_each(|(y, out)| {
        if y < h {
            out[..w * 4].copy_from_slice(&src.px[y * w * 4..(y + 1) * w * 4]);
        } else if y >= h + gap {
            let k = y - h - gap;
            let sy = h - 1 - k;
            let t = if rows > 1 { k as f32 / (rows - 1) as f32 } else { 0.0 };
            let factor = (max_a + (min_a - max_a) * t) / 255.0;
            let shift = (lean as f32 * k as f32 / rows as f32).round() as usize;
            for x in 0..w {
                let s = &src.px[(sy * w + x) * 4..][..4];
                let o = (x + shift) * 4;
                out[o..o + 3].copy_from_slice(&s[..3]);
                out[o + 3] = to_u8(f32::from(s[3]) * factor);
            }
        }
    });
    Ok(Buf { w: ow, h: oh, px })
}

/// Where the image sits on its canvas, and the canvas size.
#[derive(Clone, Copy, Debug)]
struct Canvas {
    w: usize,
    h: usize,
    /// The image's origin on the canvas.
    x0: usize,
    y0: usize,
}

impl Canvas {
    /// The canvas for `src` with an effect that reaches `reach` pixels past the image, shifted by
    /// `offset`. Without `grow` the canvas is the image's own size and the effect is clipped.
    fn for_effect(src: &Buf, reach: usize, offset: (i32, i32), grow: bool) -> Result<Canvas, EffectError> {
        if !grow {
            return Ok(Canvas { w: src.w, h: src.h, x0: 0, y0: 0 });
        }
        let (x_lo, x_hi) = span(src.w, reach, offset.0);
        let (y_lo, y_hi) = span(src.h, reach, offset.1);
        let (w, h) = canvas_size(x_hi - x_lo, y_hi - y_lo)?;
        Ok(Canvas { w, h, x0: (-x_lo) as usize, y0: (-y_lo) as usize })
    }
}

/// Extent along one axis of the image united with an effect that reaches `reach` past it and is
/// shifted by `d`, in image coordinates: `(lo, hi)` with `lo <= 0` and `hi >= len`.
fn span(len: usize, reach: usize, d: i32) -> (i64, i64) {
    let (len, reach, d) = (len as i64, reach as i64, i64::from(d));
    ((d - reach).min(0), (len + d + reach).max(len))
}

/// The canvas size if it fits a `u32` and the pixel budget, else `Invalid`.
fn canvas_size(w: i64, h: i64) -> Result<(usize, usize), EffectError> {
    let too_big = || EffectError::Invalid(format!("effect canvas {w}x{h} is too large"));
    let (Ok(w32), Ok(h32)) = (u32::try_from(w), u32::try_from(h)) else {
        return Err(too_big());
    };
    if u64::from(w32) * u64::from(h32) > MAX_CANVAS_PIXELS {
        return Err(too_big());
    }
    Ok((w32 as usize, h32 as usize))
}

/// A canvas-sized premultiplied layer: the image's alpha, blurred by `radius` over a mask padded by
/// `reach` on every side (so the blur fades out past the image), shifted by `offset`, coloured `rgb`
/// with alpha from `alpha(mask)`.
fn drop_layer(
    src: &Buf,
    cv: &Canvas,
    reach: usize,
    radius: usize,
    offset: (i32, i32),
    rgb: [f32; 3],
    alpha: impl Fn(f32) -> f32 + Sync,
) -> Result<Vec<f32>, EffectError> {
    let (mw, mh) = canvas_size((src.w + 2 * reach) as i64, (src.h + 2 * reach) as i64)?;
    let mut mask = vec![0f32; mw * mh];
    for y in 0..src.h {
        for x in 0..src.w {
            mask[(y + reach) * mw + x + reach] = f32::from(src.px[(y * src.w + x) * 4 + 3]) / 255.0;
        }
    }
    box3(&mut mask, mw, mh, 1, radius);

    let (dx, dy) = (i64::from(offset.0), i64::from(offset.1));
    let reach = reach as i64;
    let mut layer = vec![0f32; cv.w * cv.h * 4];
    layer.par_chunks_mut(cv.w * 4).enumerate().for_each(|(y, out)| {
        let v = y as i64 - cv.y0 as i64 - dy + reach;
        if v < 0 || v >= mh as i64 {
            return;
        }
        for x in 0..cv.w {
            let u = x as i64 - cv.x0 as i64 - dx + reach;
            if u < 0 || u >= mw as i64 {
                continue;
            }
            let a = unit(alpha(mask[v as usize * mw + u as usize]));
            if a > 0.0 {
                out[x * 4..x * 4 + 4].copy_from_slice(&[rgb[0] * a, rgb[1] * a, rgb[2] * a, a]);
            }
        }
    });
    Ok(layer)
}

/// Composites the image over a premultiplied canvas layer, placing the image at the canvas origin, and
/// returns the straight-alpha result.
fn over_image(mut layer: Vec<f32>, cv: &Canvas, src: &Buf) -> Buf {
    layer.par_chunks_mut(cv.w * 4).enumerate().for_each(|(y, out)| {
        if y < cv.y0 || y >= cv.y0 + src.h {
            return;
        }
        let sy = y - cv.y0;
        for sx in 0..src.w {
            let s = premultiplied(&src.px[(sy * src.w + sx) * 4..][..4]);
            let x = sx + cv.x0;
            for (d, sv) in out[x * 4..x * 4 + 4].iter_mut().zip(s) {
                *d = sv + *d * (1.0 - s[3]);
            }
        }
    });
    Buf { w: cv.w, h: cv.h, px: unpremultiply(&layer) }
}

/// The colour as 0..=1 floats, scaled by `scale`.
fn rgb_of(color: Rgba, scale: f32) -> [f32; 3] {
    [color.r, color.g, color.b].map(|c| f32::from(c) / 255.0 * scale)
}

fn alpha_of(color: Rgba) -> f32 {
    f32::from(color.a) / 255.0
}

/// Square dilation by `r`: a pixel is set when any pixel within `r` steps along either axis is set.
/// Two prefix-sum passes, so the cost doesn't depend on `r`.
fn dilate(shape: &[bool], w: usize, h: usize, r: usize) -> Vec<bool> {
    let mut rows = vec![false; w * h];
    rows.par_chunks_mut(w).zip(shape.par_chunks(w)).for_each(|(out, line)| {
        let mut prefix = vec![0usize; w + 1];
        for (x, &set) in line.iter().enumerate() {
            prefix[x + 1] = prefix[x] + usize::from(set);
        }
        for (x, o) in out.iter_mut().enumerate() {
            let (lo, hi) = (x.saturating_sub(r), x.saturating_add(r).saturating_add(1).min(w));
            *o = prefix[hi] > prefix[lo];
        }
    });
    let mut prefix = vec![0usize; (h + 1) * w];
    for y in 0..h {
        for x in 0..w {
            prefix[(y + 1) * w + x] = prefix[y * w + x] + usize::from(rows[y * w + x]);
        }
    }
    let mut out = vec![false; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, out_row)| {
        let (lo, hi) = (y.saturating_sub(r), y.saturating_add(r).saturating_add(1).min(h));
        for (x, o) in out_row.iter_mut().enumerate() {
            *o = prefix[hi * w + x] > prefix[lo * w + x];
        }
    });
    out
}
