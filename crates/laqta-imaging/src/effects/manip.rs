//! Manipulation kernels (10 effects): trimming, canvas, flips, resizing,
//! rotation, shear and rounded corners. Shared raster helpers come from `draw.rs`.

use laqta_domain::Bitmap;
use laqta_domain::color::Rgba;
use laqta_domain::effects::manip::{ForceMethod, Manip};

use super::EffectContext;
use super::EffectError;
use super::draw::{Buf, ensure_size, resize_buf, rgba, rotate_buf, rounded_coverage};
use super::manip_m7;

pub(super) fn apply(image: &Bitmap, effect: &Manip, _cx: &EffectContext<'_>) -> Result<Bitmap, EffectError> {
    debug_assert!(rayon::current_thread_index().is_some(), "effects must run inside the compute pool");
    let base = Buf::from_bitmap(image);
    let out = match *effect {
        Manip::AutoCrop { color, tolerance, margin } => auto_crop(&base, color, tolerance, margin),
        Manip::Canvas { margin, color } => set_canvas(&base, margin, color)?,
        Manip::Crop { margin } => crop(&base, margin),
        Manip::Flip { horizontal, vertical } => flip(&base, horizontal, vertical),
        Manip::ForceProportions { width, height, method, color } => force_proportions(&base, width, height, method, color)?,
        Manip::Resize { width, height, keep_aspect } => {
            let (w, h) = resize_dims(base.w, base.h, width, height, keep_aspect);
            resize_buf(&base, w, h)?
        }
        Manip::Rotate { angle, grow, color } => rotate_buf(&base, angle, grow, rgba(color))?,
        Manip::RoundedCorners { radius } => rounded_corners(&base, radius),
        Manip::Scale { width_pct, height_pct } => resize_buf(&base, percent(base.w, width_pct), percent(base.h, height_pct))?,
        Manip::Skew { x, y, grow } => skew(&base, x, y, grow)?,
        Manip::CylinderWrap { strength } => manip_m7::cylinder_wrap(&base, strength),
        Manip::DisplacementMap { amount, scale, seed } => manip_m7::displacement(&base, amount, scale, seed),
        Manip::FisheyeLens { strength } => manip_m7::fisheye(&base, strength),
        Manip::FoldCreaseWarp { angle, position, shade } => manip_m7::fold_crease(&base, angle, position, shade),
        Manip::GlassSphere { radius, refraction } => manip_m7::glass_sphere(&base, radius, refraction),
        Manip::Kaleidoscope { segments, rotation } => manip_m7::kaleidoscope(&base, segments, rotation),
        Manip::LiquifyPushSmudge { centre, radius, push, smudge } => manip_m7::liquify(&base, centre, radius, push, smudge),
        Manip::MirrorTiles { columns, rows } => manip_m7::mirror_tiles(&base, columns, rows),
        Manip::PageCurl { progress, shadow, back } => manip_m7::page_curl(&base, progress, shadow, back),
        Manip::PerspectiveWarp { top_left, top_right, bottom_right, bottom_left } => {
            manip_m7::perspective(&base, [top_left, top_right, bottom_right, bottom_left])?
        }
        Manip::PinchBulge { amount, radius } => manip_m7::pinch_bulge(&base, amount, radius),
        Manip::PolarWarp { strength } => manip_m7::polar(&base, strength),
        Manip::RippleRefraction { amplitude, wavelength } => manip_m7::ripple(&base, amplitude, wavelength),
        Manip::Rotate3D { angle_x, angle_y } => manip_m7::rotate3d(&base, angle_x, angle_y),
        Manip::Rotate3DBox { angle_x, angle_y, depth } => manip_m7::rotate3d_box(&base, angle_x, angle_y, depth),
        Manip::Symmetry { vertical, reverse } => manip_m7::symmetry(&base, vertical, reverse),
        Manip::TileRepeat { columns, rows, gap, color } => manip_m7::tile_repeat(&base, columns, rows, gap, color),
        Manip::TileShuffle { columns, rows, seed } => manip_m7::tile_shuffle(&base, columns, rows, seed),
        Manip::Twirl { angle, radius } => manip_m7::twirl(&base, angle, radius),
        Manip::WaveDistortion { amplitude, wavelength, phase } => manip_m7::wave(&base, amplitude, wavelength, phase),
    };
    out.into_bitmap()
}

/// Trims pixels that match the background. With a transparent `color` the top-left pixel is the
/// background. An image with no foreground is returned as it is.
fn auto_crop(base: &Buf, color: Rgba, tolerance: u8, margin: u32) -> Buf {
    if base.w == 0 || base.h == 0 {
        return base.clone();
    }
    let background = if color.a == 0 { base.get(0, 0) } else { rgba(color) };
    let (mut left, mut top, mut right, mut bottom) = (usize::MAX, usize::MAX, 0, 0);
    for y in 0..base.h {
        for x in 0..base.w {
            if !similar(base.get(x, y), background, tolerance) {
                left = left.min(x);
                top = top.min(y);
                right = right.max(x + 1);
                bottom = bottom.max(y + 1);
            }
        }
    }
    if right == 0 {
        return base.clone();
    }
    let m = margin as usize;
    let (x0, y0) = (left.saturating_sub(m), top.saturating_sub(m));
    let (x1, y1) = ((right + m).min(base.w), (bottom + m).min(base.h));
    base.sub(x0, y0, x1 - x0, y1 - y0)
}

/// Two pixels match when both are fully transparent, or every channel is within `tolerance`.
fn similar(a: [u8; 4], b: [u8; 4], tolerance: u8) -> bool {
    if a[3] == 0 && b[3] == 0 {
        return true;
    }
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= tolerance)
}

/// Adds (or, with negative margins, trims) pixels around the image. The image keeps its place
/// relative to the left and top edges. A result with no pixels is 1x1.
fn set_canvas(base: &Buf, margin: (i32, i32, i32, i32), color: Rgba) -> Result<Buf, EffectError> {
    let (l, t, r, b) = (i64::from(margin.0), i64::from(margin.1), i64::from(margin.2), i64::from(margin.3));
    let w = (base.w as i64 + l + r).max(1) as usize;
    let h = (base.h as i64 + t + b).max(1) as usize;
    ensure_size(w, h)?;
    let mut out = Buf::filled(w, h, rgba(color));
    out.blend(base, l, t, 1.0);
    Ok(out)
}

/// Removes (left, top, right, bottom) pixels. Nothing left is 1x1 transparent, as `ops::crop` gives.
fn crop(base: &Buf, margin: (u32, u32, u32, u32)) -> Buf {
    let (l, t, r, b) = (margin.0 as usize, margin.1 as usize, margin.2 as usize, margin.3 as usize);
    let w = base.w.saturating_sub(l).saturating_sub(r);
    let h = base.h.saturating_sub(t).saturating_sub(b);
    if w == 0 || h == 0 {
        return Buf::new(1, 1);
    }
    base.sub(l, t, w, h)
}

fn flip(base: &Buf, horizontal: bool, vertical: bool) -> Buf {
    let mut out = Buf::new(base.w, base.h);
    for y in 0..base.h {
        for x in 0..base.w {
            let sx = if horizontal { base.w - 1 - x } else { x };
            let sy = if vertical { base.h - 1 - y } else { y };
            out.set(x, y, base.get(sx, sy));
        }
    }
    out
}

/// Makes the aspect ratio `width:height`. `Grow` pads with `color`, and `Crop` trims, both centred.
fn force_proportions(base: &Buf, width: u32, height: u32, method: ForceMethod, color: Rgba) -> Result<Buf, EffectError> {
    if width == 0 || height == 0 || base.w == 0 || base.h == 0 {
        return Ok(base.clone());
    }
    let (aw, ah) = (u64::from(width), u64::from(height));
    let (w, h) = (base.w as u64, base.h as u64);
    match method {
        ForceMethod::Grow => {
            let cw = w.max((h * aw).div_ceil(ah));
            let ch = h.max((w * ah).div_ceil(aw));
            ensure_size(cw as usize, ch as usize)?;
            let mut out = Buf::filled(cw as usize, ch as usize, rgba(color));
            out.blend(base, ((cw - w) / 2) as i64, ((ch - h) / 2) as i64, 1.0);
            Ok(out)
        }
        ForceMethod::Crop => {
            let cw = w.min(h * aw / ah).max(1);
            let ch = h.min(w * ah / aw).max(1);
            Ok(base.sub(((w - cw) / 2) as usize, ((h - ch) / 2) as usize, cw as usize, ch as usize))
        }
    }
}

/// Target size of `Resize`. A zero side is automatic, see the domain type.
fn resize_dims(sw: usize, sh: usize, width: u32, height: u32, keep_aspect: bool) -> (usize, usize) {
    let (sw, sh) = (sw.max(1) as u64, sh.max(1) as u64);
    let (bw, bh) = (u64::from(width), u64::from(height));
    // Rounds half up.
    let scale = |num: u64, den: u64| (num + den / 2) / den;
    let (w, h) = if !keep_aspect {
        (if bw == 0 { sw } else { bw }, if bh == 0 { sh } else { bh })
    } else {
        match (bw, bh) {
            (0, 0) => (sw, sh),
            (bw, 0) => (bw, scale(bw * sh, sw)),
            (0, bh) => (scale(bh * sw, sh), bh),
            // Fit inside the box: whichever side reaches its bound first.
            (bw, bh) if bw * sh <= bh * sw => (bw, scale(bw * sh, sw)),
            (_, bh) => (scale(bh * sw, sh), bh),
        }
    };
    ((w.max(1)) as usize, (h.max(1)) as usize)
}

/// `len` scaled by a percentage, never below 1 pixel.
fn percent(len: usize, pct: f32) -> usize {
    ((len as f32) * pct / 100.0).round().max(1.0) as usize
}

/// Makes the corners transparent, anti-aliased. The radius is clamped to half the shorter side.
fn rounded_corners(base: &Buf, radius: u32) -> Buf {
    let mut out = base.clone();
    let (w, h, r) = (base.w as f32, base.h as f32, radius as f32);
    for y in 0..base.h {
        for x in 0..base.w {
            let cov = rounded_coverage(x as f32 + 0.5, y as f32 + 0.5, w, h, r);
            if cov < 1.0 {
                let mut p = out.get(x, y);
                p[3] = (f32::from(p[3]) * cov).round() as u8;
                out.set(x, y, p);
            }
        }
    }
    out
}

/// Horizontal and vertical shear about the centre. Each angle is clamped to ±89 degrees.
fn skew(base: &Buf, x_deg: i32, y_deg: i32, grow: bool) -> Result<Buf, EffectError> {
    let tx = f64::from(x_deg.clamp(-89, 89)).to_radians().tan();
    let ty = f64::from(y_deg.clamp(-89, 89)).to_radians().tan();
    let det = 1.0 - tx * ty;
    if det.abs() < 1e-9 {
        return Err(EffectError::Invalid("skew angles cancel out".to_owned()));
    }
    let (sw, sh) = (base.w as f64, base.h as f64);
    // The shear maps (x, y) to (x + tx * y, ty * x + y). The corners give the bounding box.
    let (dw, dh) = if grow {
        let ex = sw / 2.0 + tx.abs() * sh / 2.0;
        let ey = ty.abs() * sw / 2.0 + sh / 2.0;
        (((2.0 * ex) - 1e-9).ceil().max(0.0) as usize, ((2.0 * ey) - 1e-9).ceil().max(0.0) as usize)
    } else {
        (base.w, base.h)
    };
    ensure_size(dw, dh)?;
    let mut out = Buf::new(dw, dh);
    for y in 0..dh {
        for x in 0..dw {
            let px = x as f64 + 0.5 - dw as f64 / 2.0;
            let py = y as f64 + 0.5 - dh as f64 / 2.0;
            // Inverse of [[1, tx], [ty, 1]] is [[1, -tx], [-ty, 1]] / det.
            let sx = (px - tx * py) / det + sw / 2.0;
            let sy = (py - ty * px) / det + sh / 2.0;
            out.set(x, y, base.sample(sx, sy));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn resize_keep_aspect_math() {
        assert_eq!(resize_dims(20, 16, 10, 0, true), (10, 8));
        assert_eq!(resize_dims(20, 16, 0, 4, true), (5, 4));
        assert_eq!(resize_dims(20, 16, 10, 10, true), (10, 8));
        assert_eq!(resize_dims(20, 16, 40, 10, true), (13, 10));
        assert_eq!(resize_dims(20, 16, 10, 10, false), (10, 10));
        assert_eq!(resize_dims(20, 16, 0, 0, true), (20, 16));
        assert_eq!(resize_dims(20, 16, 0, 0, false), (20, 16));
    }

    #[test]
    fn force_proportions_dims() {
        let base = Buf::filled(20, 16, [1, 2, 3, 255]);
        let grow = force_proportions(&base, 1, 1, ForceMethod::Grow, Rgba::TRANSPARENT).unwrap();
        assert_eq!((grow.w, grow.h), (20, 20));
        let crop = force_proportions(&base, 1, 1, ForceMethod::Crop, Rgba::TRANSPARENT).unwrap();
        assert_eq!((crop.w, crop.h), (16, 16));
    }

    #[test]
    fn skew_with_zero_angles_keeps_pixels() {
        let base = Buf::filled(5, 4, [9, 8, 7, 255]);
        let out = skew(&base, 0, 0, true).unwrap();
        assert_eq!((out.w, out.h), (5, 4));
        assert_eq!(out.px, base.px);
    }

    #[test]
    fn skew_at_forty_five_each_way_is_rejected() {
        let base = Buf::filled(5, 4, [9, 8, 7, 255]);
        assert!(matches!(skew(&base, 45, 45, true), Err(EffectError::Invalid(_))));
    }

    #[test]
    fn similar_treats_transparent_pixels_as_equal() {
        assert!(similar([1, 2, 3, 0], [9, 9, 9, 0], 0));
        assert!(!similar([1, 2, 3, 255], [1, 2, 9, 255], 3));
        assert!(similar([1, 2, 3, 255], [1, 2, 6, 255], 3));
    }
}
