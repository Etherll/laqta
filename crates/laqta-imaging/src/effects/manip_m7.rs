//! Kernels for the 20 M7 manipulations (X1 batch, M7 D5): warps, mirrors, tiles and 3D turns.
//! Each output pixel is computed from its own coordinates, so rows run on the compute pool and the
//! result doesn't depend on the thread count. Sampling is bilinear (`Buf::sample`), and points outside
//! the input are transparent. Angles are degrees unless a name says otherwise.

use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI, SQRT_2, TAU};

use laqta_domain::color::Rgba;
use rayon::prelude::*;

use super::EffectError;
use super::draw::{Buf, SplitMix64, hash_unit, rgba};

/// Builds a `w` x `h` buffer from `f(x, y)`, one row per task.
fn build(w: usize, h: usize, f: impl Fn(usize, usize) -> [u8; 4] + Sync) -> Buf {
    let mut px = vec![0u8; w * h * 4];
    px.par_chunks_mut((w * 4).max(1)).enumerate().for_each(|(y, row)| {
        for (x, p) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            *p = f(x, y);
        }
    });
    Buf { w, h, px }
}

/// The image centre, in pixels.
fn centre(base: &Buf) -> (f64, f64) {
    (base.w as f64 / 2.0, base.h as f64 / 2.0)
}

/// The centre of pixel (`x`, `y`), relative to the image centre.
fn offset(x: usize, y: usize, c: (f64, f64)) -> (f64, f64) {
    (x as f64 + 0.5 - c.0, y as f64 + 0.5 - c.1)
}

/// `r` pixels, or half the shorter side when `r` is zero. Never below 1.
fn radius_or_auto(base: &Buf, r: u32) -> f64 {
    if r == 0 { (base.w.min(base.h) as f64 / 2.0).max(1.0) } else { f64::from(r) }
}

/// Scales the colour channels of a pixel, keeping alpha. `k` is 0..=1.
fn scale_rgb(p: [u8; 4], k: f64) -> [u8; 4] {
    [(f64::from(p[0]) * k).round() as u8, (f64::from(p[1]) * k).round() as u8, (f64::from(p[2]) * k).round() as u8, p[3]]
}

/// Wraps the image round a vertical cylinder. `strength` is the share of a half turn the picture
/// spans: the edges stay at the edges and the centre stretches.
pub(super) fn cylinder_wrap(base: &Buf, strength: f32) -> Buf {
    let arc = f64::from(strength.clamp(0.0, 1.0)) * FRAC_PI_2 * 0.98;
    if arc <= 1e-9 {
        return base.clone();
    }
    let sin_max = arc.sin();
    let (w, h) = (base.w, base.h);
    build(w, h, |x, y| {
        let u = (x as f64 + 0.5) / w as f64 * 2.0 - 1.0;
        let src_u = (u * sin_max).asin() / arc;
        base.sample((src_u + 1.0) / 2.0 * w as f64, y as f64 + 0.5)
    })
}

/// Smooth value noise in 0..1: random values on a lattice of `cell`-pixel squares, blended with a
/// smoothstep. `channel` picks an independent lattice.
fn value_noise(seed: u64, channel: u64, x: f64, y: f64, cell: f64) -> f64 {
    let (gx, gy) = (x / cell, y / cell);
    let (ix, iy) = (gx.floor(), gy.floor());
    let (fx, fy) = (gx - ix, gy - iy);
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let seed = seed.wrapping_add(channel.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    let lattice = |i: f64, j: f64| hash_unit(seed, i as i64 as u64, j as i64 as u64);
    let top = lattice(ix, iy) + (lattice(ix + 1.0, iy) - lattice(ix, iy)) * sx;
    let bottom = lattice(ix, iy + 1.0) + (lattice(ix + 1.0, iy + 1.0) - lattice(ix, iy + 1.0)) * sx;
    top + (bottom - top) * sy
}

/// Moves each pixel by up to `amount` pixels, using seeded value noise on `scale`-pixel cells for x
/// and a second lattice for y.
pub(super) fn displacement(base: &Buf, amount: u32, scale: u32, seed: u64) -> Buf {
    let amp = f64::from(amount);
    let cell = f64::from(scale.max(1));
    build(base.w, base.h, |x, y| {
        let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
        let dx = amp * (2.0 * value_noise(seed, 0, px, py, cell) - 1.0);
        let dy = amp * (2.0 * value_noise(seed, 1, px, py, cell) - 1.0);
        base.sample(px + dx, py + dy)
    })
}

/// A fisheye lens. The source radius is tan(a r) / tan(a), with r the distance over the inscribed
/// circle's radius: the centre is magnified and the rim is fixed. Outside the circle is unchanged.
pub(super) fn fisheye(base: &Buf, strength: f32) -> Buf {
    let a = f64::from(strength.clamp(0.0, 1.0)) * FRAC_PI_4 * 0.98;
    if a <= 1e-9 {
        return base.clone();
    }
    let c = centre(base);
    let radius = base.w.min(base.h) as f64 / 2.0;
    let tan_a = a.tan();
    build(base.w, base.h, |x, y| {
        let (dx, dy) = offset(x, y, c);
        let d = dx.hypot(dy);
        if d >= radius {
            return base.get(x, y);
        }
        let k = (a * d / radius).tan() / tan_a * radius / d.max(1e-12);
        base.sample(c.0 + dx * k, c.1 + dy * k)
    })
}

/// Folds one side of a line over the other. The line's normal is `angle` degrees, and the line sits
/// `position` (0..1) across the image. The folded flap shows the mirror image, darkened at the crease.
pub(super) fn fold_crease(base: &Buf, angle: f32, position: f32, shade: f32) -> Buf {
    let c = centre(base);
    let (sin, cos) = f64::from(angle).to_radians().sin_cos();
    let normal = [cos, sin];
    let extent = (base.w as f64 * cos.abs() + base.h as f64 * sin.abs()) / 2.0;
    let line_at = (f64::from(position) - 0.5) * 2.0 * extent;
    let band = (0.1 * extent).max(1.0);
    let k = f64::from(shade.clamp(0.0, 1.0));
    build(base.w, base.h, |x, y| {
        let (dx, dy) = offset(x, y, c);
        let d = dx * normal[0] + dy * normal[1] - line_at;
        if d > 0.0 {
            let fx = c.0 + dx - 2.0 * d * normal[0];
            let fy = c.1 + dy - 2.0 * d * normal[1];
            let fade = (1.0 - d / band).clamp(0.0, 1.0) * k;
            return scale_rgb(base.sample(fx, fy), 1.0 - fade);
        }
        let px = base.get(x, y);
        if d > -band {
            let shadow = (1.0 + d / band).clamp(0.0, 1.0) * k * 0.5;
            return scale_rgb(px, 1.0 - shadow);
        }
        px
    })
}

fn normalise(v: [f64; 3]) -> [f64; 3] {
    let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    v.map(|c| c / n)
}

/// A glass ball over the middle. Inside it, pixels are taken from closer to the centre, which
/// magnifies, and a highlight sits at the upper left. Outside it, the image is unchanged.
pub(super) fn glass_sphere(base: &Buf, radius: u32, refraction: f32) -> Buf {
    let r = radius_or_auto(base, radius);
    let s = f64::from(refraction.clamp(0.0, 1.0));
    let c = centre(base);
    let light = normalise([-0.4, -0.5, 0.8]);
    build(base.w, base.h, |x, y| {
        let (dx, dy) = offset(x, y, c);
        let q = dx.hypot(dy) / r;
        if q >= 1.0 {
            return base.get(x, y);
        }
        let z = (1.0 - q * q).sqrt();
        let k = 1.0 - s * z;
        let px = base.sample(c.0 + dx * k, c.1 + dy * k);
        let spec = (dx / r * light[0] + dy / r * light[1] + z * light[2]).max(0.0).powi(24) * 0.6;
        let lift = |v: u8| (f64::from(v) + (255.0 - f64::from(v)) * spec).round() as u8;
        [lift(px[0]), lift(px[1]), lift(px[2]), px[3]]
    })
}

/// `segments` mirrored wedges around the centre, turned by `rotation` degrees.
pub(super) fn kaleidoscope(base: &Buf, segments: u32, rotation: f32) -> Buf {
    let seg = TAU / f64::from(segments.max(2));
    let rot = f64::from(rotation).to_radians();
    let c = centre(base);
    build(base.w, base.h, |x, y| {
        let (dx, dy) = offset(x, y, c);
        let r = dx.hypot(dy);
        let mut a = (dy.atan2(dx) - rot).rem_euclid(seg);
        if a > seg / 2.0 {
            a = seg - a;
        }
        let t = a + rot;
        base.sample(c.0 + r * t.cos(), c.1 + r * t.sin())
    })
}

/// Liquify push and smudge. Pixels move by `push` pixels at `centre` (given as fractions of the
/// image), and the push fades to nothing at `radius`. `smudge` mixes in the average along the push.
pub(super) fn liquify(base: &Buf, centre_frac: (f32, f32), radius: u32, push: (f32, f32), smudge: f32) -> Buf {
    let at = (f64::from(centre_frac.0) * base.w as f64, f64::from(centre_frac.1) * base.h as f64);
    let r = f64::from(radius.max(1));
    let (push_x, push_y) = (f64::from(push.0), f64::from(push.1));
    let mix = f64::from(smudge.clamp(0.0, 1.0));
    build(base.w, base.h, |x, y| {
        let (x0, y0) = (x as f64 + 0.5, y as f64 + 0.5);
        let d = (x0 - at.0).hypot(y0 - at.1);
        if d >= r {
            return base.get(x, y);
        }
        let f = (1.0 - d / r).powi(2);
        let (dx, dy) = (push_x * f, push_y * f);
        let pushed = base.sample(x0 - dx, y0 - dy);
        if mix <= 0.0 {
            return pushed;
        }
        let mut along = [0.0f64; 4];
        for step in 1..=4 {
            let t = f64::from(step) / 4.0;
            for (a, v) in along.iter_mut().zip(base.sample(x0 - dx * t, y0 - dy * t)) {
                *a += f64::from(v) / 4.0;
            }
        }
        std::array::from_fn(|c| {
            let p = f64::from(pushed[c]);
            (p + (along[c] - p) * mix).round() as u8
        })
    })
}

/// Tiles the image `columns` by `rows`, mirroring every other tile, so each seam is a reflection.
pub(super) fn mirror_tiles(base: &Buf, columns: u32, rows: u32) -> Buf {
    let (cols, rws) = (f64::from(columns.max(1)), f64::from(rows.max(1)));
    let (w, h) = (base.w as f64, base.h as f64);
    build(base.w, base.h, |x, y| {
        let fx = (x as f64 + 0.5) / w * cols;
        let fy = (y as f64 + 0.5) / h * rws;
        let (tx, ty) = (fx.floor(), fy.floor());
        let mut u = fx - tx;
        let mut v = fy - ty;
        if !(tx as u64).is_multiple_of(2) {
            u = 1.0 - u;
        }
        if !(ty as u64).is_multiple_of(2) {
            v = 1.0 - v;
        }
        base.sample(u * w, v * h)
    })
}

/// Folds the bottom-right corner back along a diagonal. The flap is the page's back colour, with a
/// little of the reflected image through it, darker at the fold. The page under the fold is shaded by
/// `shadow`.
pub(super) fn page_curl(base: &Buf, progress: f32, shadow: f32, back: Rgba) -> Buf {
    let p = f64::from(progress.clamp(0.0, 1.0));
    if p <= 0.0 {
        return base.clone();
    }
    let extent = (base.w + base.h) as f64 / SQRT_2;
    let fold = extent * (1.0 - p);
    let band = (0.06 * extent).max(1.0);
    let k = f64::from(shadow.clamp(0.0, 1.0));
    let paper = [back.r, back.g, back.b].map(f64::from);
    build(base.w, base.h, |x, y| {
        let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
        let proj = (px + py) / SQRT_2;
        let here = base.get(x, y);
        if proj > fold {
            // Reflect across the fold line, whose normal is (1, 1) / sqrt(2).
            let d = proj - fold;
            let reflected = base.sample(px - SQRT_2 * d, py - SQRT_2 * d);
            let mix = 0.15 * f64::from(reflected[3]) / 255.0;
            let shade = 0.82 + 0.18 * (d / band).min(1.0);
            let rgb: [u8; 3] = std::array::from_fn(|c| ((paper[c] * (1.0 - mix) + f64::from(reflected[c]) * mix) * shade).round() as u8);
            return [rgb[0], rgb[1], rgb[2], 255];
        }
        if proj > fold - band {
            return scale_rgb(here, 1.0 - (1.0 - (fold - proj) / band) * k * 0.6);
        }
        here
    })
}

/// The projective map sending each `from[i]` to `to[i]`, as eight coefficients with h33 = 1. `None`
/// when the points are degenerate.
fn homography(from: [(f64, f64); 4], to: [(f64, f64); 4]) -> Option<[f64; 8]> {
    let mut m = [[0.0f64; 9]; 8];
    for (i, (&(x, y), &(u, v))) in from.iter().zip(&to).enumerate() {
        m[2 * i] = [x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y, u];
        m[2 * i + 1] = [0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y, v];
    }
    for col in 0..8 {
        let pivot = (col..8).max_by(|&a, &b| m[a][col].abs().total_cmp(&m[b][col].abs()))?;
        if m[pivot][col].abs() < 1e-12 {
            return None;
        }
        m.swap(col, pivot);
        let p = m[col][col];
        for v in &mut m[col] {
            *v /= p;
        }
        let row = m[col];
        for (r, other) in m.iter_mut().enumerate() {
            if r != col {
                let f = other[col];
                for (v, rv) in other.iter_mut().zip(row) {
                    *v -= f * rv;
                }
            }
        }
    }
    Some(std::array::from_fn(|k| m[k][8]))
}

/// Maps the image onto a quadrilateral. The corners are fractions of the image size, in the order
/// top left, top right, bottom right, bottom left. Each output pixel is sampled through the inverse
/// projective map, and points outside the quadrilateral are transparent.
pub(super) fn perspective(base: &Buf, corners: [(f32, f32); 4]) -> Result<Buf, EffectError> {
    let (w, h) = (base.w as f64, base.h as f64);
    let quad = corners.map(|(x, y)| (f64::from(x) * w, f64::from(y) * h));
    let image = [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)];
    let Some(k) = homography(quad, image) else {
        return Err(EffectError::Invalid("the corners don't make a quadrilateral".to_owned()));
    };
    Ok(build(base.w, base.h, |x, y| {
        let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
        let den = k[6] * px + k[7] * py + 1.0;
        if den <= 1e-12 {
            return [0; 4];
        }
        base.sample((k[0] * px + k[1] * py + k[2]) / den, (k[3] * px + k[4] * py + k[5]) / den)
    }))
}

/// Pinches (`amount` > 0) or bulges (< 0) about the centre. The sample distance is the distance times
/// (1 + amount (1 - t)^2), with t the distance over the radius, so the rim stays fixed.
pub(super) fn pinch_bulge(base: &Buf, amount: f32, radius: u32) -> Buf {
    let a = f64::from(amount.clamp(-1.0, 1.0));
    let r = radius_or_auto(base, radius);
    let c = centre(base);
    build(base.w, base.h, |x, y| {
        let (dx, dy) = offset(x, y, c);
        let d = dx.hypot(dy);
        if d >= r {
            return base.get(x, y);
        }
        let k = 1.0 + a * (1.0 - d / r).powi(2);
        base.sample(c.0 + dx * k, c.1 + dy * k)
    })
}

/// Unwraps the image into polar coordinates: x becomes the angle and y the distance from the centre,
/// so the top row lies on the rim. `strength` blends from the flat image to the full warp.
pub(super) fn polar(base: &Buf, strength: f32) -> Buf {
    let t = f64::from(strength.clamp(0.0, 1.0));
    if t <= 0.0 {
        return base.clone();
    }
    let (w, h) = (base.w as f64, base.h as f64);
    let c = centre(base);
    let rim = c.0.hypot(c.1).max(1.0);
    build(base.w, base.h, |x, y| {
        let (x0, y0) = (x as f64 + 0.5, y as f64 + 0.5);
        let (dx, dy) = (x0 - c.0, y0 - c.1);
        let px = (dy.atan2(dx) + PI) / TAU * w;
        let py = dx.hypot(dy) / rim * h;
        base.sample(x0 + (px.rem_euclid(w) - x0) * t, y0 + (py - y0) * t)
    })
}

/// Concentric ripples. Each pixel moves along its radius by `amplitude` pixels times a sine of its
/// distance from the centre, `wavelength` pixels per crest.
pub(super) fn ripple(base: &Buf, amplitude: u32, wavelength: u32) -> Buf {
    let amp = f64::from(amplitude);
    let wl = f64::from(wavelength.max(1));
    let c = centre(base);
    build(base.w, base.h, |x, y| {
        let (dx, dy) = offset(x, y, c);
        let d = dx.hypot(dy);
        if d == 0.0 {
            return base.get(x, y);
        }
        let push = amp * (TAU * d / wl).sin() / d;
        let (x0, y0) = (x as f64 + 0.5, y as f64 + 0.5);
        base.sample(x0 + dx * push, y0 + dy * push)
    })
}

/// A 3x3 matrix, row-major.
type Mat3 = [[f64; 3]; 3];

/// Turns about x by `angle_x` degrees, then about y by `angle_y` degrees.
fn rotation(angle_x: f32, angle_y: f32) -> Mat3 {
    let (sx, cx) = f64::from(angle_x).to_radians().sin_cos();
    let (sy, cy) = f64::from(angle_y).to_radians().sin_cos();
    let rot_x = [[1.0, 0.0, 0.0], [0.0, cx, -sx], [0.0, sx, cx]];
    let rot_y = [[cy, 0.0, sy], [0.0, 1.0, 0.0], [-sy, 0.0, cy]];
    mat_mul(&rot_y, &rot_x)
}

fn mat_mul(a: &Mat3, b: &Mat3) -> Mat3 {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum::<f64>()))
}

fn transpose(m: &Mat3) -> Mat3 {
    std::array::from_fn(|i| std::array::from_fn(|j| m[j][i]))
}

fn mat_vec(m: &Mat3, v: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

/// The camera ray through the image at offset (`sx`, `sy`), from a camera `dist` in front of the
/// image. Returns origin and direction in the image's own frame, where the image is the z = 0 plane.
fn ray_in_frame(inv: &Mat3, sx: f64, sy: f64, dist: f64) -> ([f64; 3], [f64; 3]) {
    (mat_vec(inv, [0.0, 0.0, -dist]), mat_vec(inv, [sx, sy, dist]))
}

/// Turns the image in 3D, with perspective. Each pixel's camera ray is intersected with the turned
/// image plane, and the hit point is sampled. Rays that miss are transparent.
pub(super) fn rotate3d(base: &Buf, angle_x: f32, angle_y: f32) -> Buf {
    let c = centre(base);
    let dist = 2.0 * base.w.max(base.h) as f64;
    let inv = transpose(&rotation(angle_x, angle_y));
    build(base.w, base.h, |x, y| {
        let (sx, sy) = offset(x, y, c);
        let (o, d) = ray_in_frame(&inv, sx, sy, dist);
        if d[2].abs() < 1e-12 {
            return [0; 4];
        }
        let t = -o[2] / d[2];
        if t <= 0.0 {
            return [0; 4];
        }
        base.sample(c.0 + o[0] + t * d[0], c.1 + o[1] + t * d[1])
    })
}

/// A box `depth` pixels thick, with the image on its front, turned in 3D. Each ray enters the box
/// at its nearest face: the front shows the image, the back shows it mirrored and darker, and the
/// sides show the image's edge pixels.
pub(super) fn rotate3d_box(base: &Buf, angle_x: f32, angle_y: f32, depth: u32) -> Buf {
    let c = centre(base);
    let dist = 2.0 * base.w.max(base.h) as f64;
    let inv = transpose(&rotation(angle_x, angle_y));
    let half = [c.0, c.1, f64::from(depth) / 2.0];
    let (w, h) = (base.w, base.h);
    build(w, h, |x, y| {
        let (sx, sy) = offset(x, y, c);
        let (o, d) = ray_in_frame(&inv, sx, sy, dist);
        // Slab test against the box, keeping the entry face.
        let (mut t_near, mut t_far, mut face) = (f64::NEG_INFINITY, f64::INFINITY, 2);
        for (k, ((&dk, &ok), &hk)) in d.iter().zip(o.iter()).zip(half.iter()).enumerate() {
            if dk.abs() < 1e-12 {
                if ok.abs() > hk {
                    return [0; 4];
                }
                continue;
            }
            let (a, b) = ((-hk - ok) / dk, (hk - ok) / dk);
            let (near, far) = if a < b { (a, b) } else { (b, a) };
            if near > t_near {
                t_near = near;
                face = k;
            }
            t_far = t_far.min(far);
        }
        if t_near > t_far || t_far <= 0.0 {
            return [0; 4];
        }
        let hit = [o[0] + t_near * d[0], o[1] + t_near * d[1], o[2] + t_near * d[2]];
        let index = |v: f64, n: usize| (v.floor().max(0.0) as usize).min(n - 1);
        let (ix, iy) = (index(hit[0] + c.0, w), index(hit[1] + c.1, h));
        match face {
            2 if hit[2] > 0.0 => base.get(ix, iy),
            2 => scale_rgb(base.get(w - 1 - ix, iy), 0.5),
            0 => scale_rgb(base.get(if hit[0] > 0.0 { w - 1 } else { 0 }, iy), 0.6),
            _ => scale_rgb(base.get(ix, if hit[1] > 0.0 { h - 1 } else { 0 }), 0.8),
        }
    })
}

/// Copies one half over the other. `vertical` mirrors the left half onto the right, otherwise the
/// top onto the bottom. `reverse` takes the other half as the source.
pub(super) fn symmetry(base: &Buf, vertical: bool, reverse: bool) -> Buf {
    build(base.w, base.h, |x, y| {
        let (along, n) = if vertical { (x, base.w) } else { (y, base.h) };
        let keep = if reverse { along >= n.div_ceil(2) } else { along < n / 2 };
        let src = if keep { along } else { n - 1 - along };
        if vertical { base.get(src, y) } else { base.get(x, src) }
    })
}

/// Repeats the whole image, scaled to a tile, in `columns` by `rows` cells. `gap` pixels of `color`
/// separate the tiles.
pub(super) fn tile_repeat(base: &Buf, columns: u32, rows: u32, gap: u32, color: Rgba) -> Buf {
    let (cols, rws) = (f64::from(columns.max(1)), f64::from(rows.max(1)));
    let g = f64::from(gap);
    let (w, h) = (base.w as f64, base.h as f64);
    let tile_w = ((w - (cols - 1.0) * g) / cols).max(1.0);
    let tile_h = ((h - (rws - 1.0) * g) / rws).max(1.0);
    let (pitch_x, pitch_y) = (tile_w + g, tile_h + g);
    let fill = rgba(color);
    build(base.w, base.h, |x, y| {
        let (x0, y0) = (x as f64 + 0.5, y as f64 + 0.5);
        let (tx, ty) = ((x0 / pitch_x).floor(), (y0 / pitch_y).floor());
        if tx >= cols || ty >= rws {
            return fill;
        }
        let (lx, ly) = (x0 - tx * pitch_x, y0 - ty * pitch_y);
        if lx >= tile_w || ly >= tile_h {
            return fill;
        }
        base.sample(lx / tile_w * w, ly / tile_h * h)
    })
}

/// Cuts the image into `columns` by `rows` tiles and shuffles them, in the order `seed` gives. Any
/// remainder pixels on the right and bottom stay where they are.
pub(super) fn tile_shuffle(base: &Buf, columns: u32, rows: u32, seed: u64) -> Buf {
    let cols = columns.max(1) as usize;
    let rws = rows.max(1) as usize;
    let (tw, th) = (base.w / cols, base.h / rws);
    if tw == 0 || th == 0 {
        return base.clone();
    }
    let mut order: Vec<usize> = (0..cols * rws).collect();
    let mut rng = SplitMix64(seed);
    for i in (1..order.len()).rev() {
        let j = rng.below(i as u64 + 1) as usize;
        order.swap(i, j);
    }
    let mut out = base.clone();
    for (dst, &src) in order.iter().enumerate() {
        let (dx0, dy0) = ((dst % cols) * tw, (dst / cols) * th);
        let (sx0, sy0) = ((src % cols) * tw, (src / cols) * th);
        for y in 0..th {
            for x in 0..tw {
                out.set(dx0 + x, dy0 + y, base.get(sx0 + x, sy0 + y));
            }
        }
    }
    out
}

/// Turns pixels about the centre. The angle is `angle` degrees at the centre and eases to no turn at
/// `radius` (0 is half the shorter side).
pub(super) fn twirl(base: &Buf, angle: f32, radius: u32) -> Buf {
    let r = radius_or_auto(base, radius);
    let a = f64::from(angle).to_radians();
    let c = centre(base);
    build(base.w, base.h, |x, y| {
        let (dx, dy) = offset(x, y, c);
        let d = dx.hypot(dy);
        if d >= r {
            return base.get(x, y);
        }
        let f = 1.0 - d / r;
        let (s, co) = (a * f * f).sin_cos();
        base.sample(c.0 + dx * co - dy * s, c.1 + dx * s + dy * co)
    })
}

/// Shifts each row sideways by a sine of its height, so the image undulates. `amplitude` and
/// `wavelength` are in pixels, and `phase` in radians.
pub(super) fn wave(base: &Buf, amplitude: u32, wavelength: u32, phase: f32) -> Buf {
    let amp = f64::from(amplitude);
    let wl = f64::from(wavelength.max(1));
    let ph = f64::from(phase);
    build(base.w, base.h, |x, y| {
        let y0 = y as f64 + 0.5;
        let shift = amp * (TAU * y0 / wl + ph).sin();
        base.sample(x as f64 + 0.5 + shift, y0)
    })
}
