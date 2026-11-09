//! Filters: frames, glitch, optics, prints and scenes. Each kernel is written from the
//! effect's name and what it does to an image, and shares the helpers in the parent module. Pixels are
//! straight RGBA8 in and out. Every kernel is pure and deterministic for its seed.

use std::f32::consts::{PI, TAU};

use laqta_domain::color::Rgba;
use rayon::prelude::*;

use super::{Buf, Rng, blur, cap, cells, hash, shifted, to_u8, unit};
use crate::effects::EffectError;

/// Largest canvas an effect may allocate, in pixels. Matches the budget in `canvas.rs`.
const MAX_CANVAS_PIXELS: u64 = 1 << 28;
/// Caps on parameters that would otherwise ask for unbounded work. Larger values render as the cap.
const MAX_CELL: u32 = 4096;
const MAX_BLOCK: u32 = 256;
const MAX_EDGE: u32 = 256;
const MAX_FROST_RADIUS: u32 = 32;
const MAX_GHOSTS: u32 = 32;
const MAX_GLARE: u32 = 48;
const MAX_GLITCH_BANDS: u32 = 512;
const MAX_HALO_RADIUS: u32 = 128;
const MAX_HAZE: u32 = 64;
const MAX_HEX_SIZE: u32 = 1 << 16;
/// Hexagon cells are enlarged until the grid is at most this many cells across.
const MAX_HEX_GRID: f32 = 1024.0;
const MAX_INK_SPLATS: u32 = 32;
const MAX_LENS_RADIUS: u32 = 24;
const MAX_SHADOW_REACH: u32 = 256;
const MAX_MINI_BLUR: u32 = 64;
const MAX_RAIN_CELL: u32 = 64;
const MAX_SCAN_SPACING: u32 = 64;
const MAX_SHIMMER_PERIOD: u32 = 4096;
const MAX_GLASS: u32 = 64;

/// Fixed sizes of the kernels.
const BLOCK: usize = 8;
const LENS_TAPS: usize = 48;
const INK_DROPS: usize = 24;
const SCATTER: u64 = 8;
/// Golden angle in radians: successive taps of a spiral spread evenly over a disc.
const GOLDEN_ANGLE: f32 = 2.399_963;
const SQRT_3: f32 = 1.732_050_8;

/// Per-effect salts, so two effects given one seed don't draw the same numbers.
const FROST_SALT: u64 = 0xF805_7001;
const EDGE_SALT: u64 = 0x1CE0_7002;
const GLITCH_SALT: u64 = 0x611C_7003;
const INK_SALT: u64 = 0x1B57_7004;
const RAIN_SALT: u64 = 0x2A10_7005;
const GLYPH_SALT: u64 = 0x6109_7006;
const SCAN_SALT: u64 = 0x5CA7_7007;
const LAVA_SALT: u64 = 0x1A7A_7008;
const GLASS_SALT: u64 = 0x61A5_7009;
const OIL_SALT: u64 = 0x0117_700A;
const PAPER_SALT: u64 = 0xFA9E_700B;
const STAR_SALT: u64 = 0x57A2_700C;
const GRASS_SALT: u64 = 0x6A55_700D;
const DIRT_SALT: u64 = 0xD187_700E;

// ---- Shared helpers ----

/// Runs `f` for every pixel of an image the size of `src`, rows in parallel.
fn per_pixel(src: &Buf, f: impl Fn(usize, usize) -> [u8; 4] + Sync) -> Buf {
    let w = src.w;
    let mut px = vec![0u8; src.px.len()];
    px.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for (x, out) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            *out = f(x, y);
        }
    });
    Buf { w, h: src.h, px }
}

/// The canvas size for a grown image, or `Invalid` past the pixel budget.
fn fit(cw: u64, ch: u64) -> Result<(usize, usize), EffectError> {
    if cw > u64::from(u32::MAX) || ch > u64::from(u32::MAX) || cw * ch > MAX_CANVAS_PIXELS {
        return Err(EffectError::Invalid(format!("a {cw} x {ch} canvas is past the pixel budget")));
    }
    Ok((cw as usize, ch as usize))
}

/// Runs `paint` on the margin of the canvas and copies the image into the middle. `paint` gets the canvas
/// position and the depth, the distance from the canvas edge.
fn frame(src: &Buf, width: u32, paint: impl Fn(usize, usize, usize) -> [u8; 4] + Sync) -> Result<Buf, EffectError> {
    let margin = width as usize;
    let (cw, ch) = fit(src.w as u64 + 2 * u64::from(width), src.h as u64 + 2 * u64::from(width))?;
    let mut px = vec![0u8; cw * ch * 4];
    px.par_chunks_mut(cw * 4).enumerate().for_each(|(y, row)| {
        for (x, out) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let depth = x.min(y).min(cw - 1 - x).min(ch - 1 - y);
            *out = if depth < margin { paint(x, y, depth) } else { px_at(src, x - margin, y - margin) };
        }
    });
    Ok(Buf { w: cw, h: ch, px })
}

fn px_at(src: &Buf, x: usize, y: usize) -> [u8; 4] {
    let i = (y * src.w + x) * 4;
    [src.px[i], src.px[i + 1], src.px[i + 2], src.px[i + 3]]
}

/// The pixel under `p`, clamped to the image.
fn sample(src: &Buf, p: [f32; 2]) -> [u8; 4] {
    let x = p[0].floor().clamp(0.0, (src.w - 1) as f32) as usize;
    let y = p[1].floor().clamp(0.0, (src.h - 1) as f32) as usize;
    px_at(src, x, y)
}

/// The pixel at (`x`, `y`), or transparent black outside the image.
fn at_or_clear(src: &Buf, x: i64, y: i64) -> [u8; 4] {
    if (0..src.w as i64).contains(&x) && (0..src.h as i64).contains(&y) { px_at(src, x as usize, y as usize) } else { [0; 4] }
}

fn rgb3(p: [u8; 4]) -> [f32; 3] {
    [f32::from(p[0]), f32::from(p[1]), f32::from(p[2])]
}

fn rgb_of(c: Rgba) -> [f32; 3] {
    [f32::from(c.r), f32::from(c.g), f32::from(c.b)]
}

fn rgba4(c: Rgba) -> [u8; 4] {
    [c.r, c.g, c.b, c.a]
}

/// Straight RGBA from float channels (rounded and clamped), with the given alpha.
fn pack(rgb: [f32; 3], alpha: u8) -> [u8; 4] {
    [to_u8(rgb[0]), to_u8(rgb[1]), to_u8(rgb[2]), alpha]
}

/// The colour scaled by `k`, alpha kept.
fn shade(c: Rgba, k: f32) -> [u8; 4] {
    pack(rgb_of(c).map(|v| v * k), c.a)
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    std::array::from_fn(|c| a[c] + (b[c] - a[c]) * t)
}

/// One channel screened over `base` by `top` (0..=1), in float 0..=255.
fn screen(base: u8, top: f32) -> f32 {
    let top = top.clamp(0.0, 1.0);
    255.0 * (1.0 - (1.0 - f32::from(base) / 255.0) * (1.0 - top))
}

/// Rec. 601 luma of a straight pixel, 0..=1.
fn luma(p: &[u8]) -> f32 {
    (0.299 * f32::from(p[0]) + 0.587 * f32::from(p[1]) + 0.114 * f32::from(p[2])) / 255.0
}

/// Hermite step from `e0` to `e1`, clamped to 0..=1. Works with `e0 > e1` too.
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let span = e1 - e0;
    let t = if span.abs() < f32::EPSILON { if x < e0 { 0.0 } else { 1.0 } } else { ((x - e0) / span).clamp(0.0, 1.0) };
    t * t * (3.0 - 2.0 * t)
}

/// Uniform in `0..1` from `(seed, index)`, from the top 24 bits of the hash.
fn unit_hash(seed: u64, index: u64) -> f32 {
    (hash(seed, index) >> 40) as f32 / 16_777_216.0
}

/// Value noise: hashed values on a lattice of unit cells, blended with a smooth weight.
fn value_noise(seed: u64, x: f32, y: f32) -> f32 {
    let (x0, y0) = (x.floor(), y.floor());
    let (u, v) = (smoothstep(0.0, 1.0, x - x0), smoothstep(0.0, 1.0, y - y0));
    let (ix, iy) = (x0 as i64, y0 as i64);
    let corner = |dx: i64, dy: i64| unit_hash(seed, (((ix + dx) as u64) << 32) ^ ((iy + dy) as u64 & 0xffff_ffff));
    let top = corner(0, 0) + (corner(1, 0) - corner(0, 0)) * u;
    let bottom = corner(0, 1) + (corner(1, 1) - corner(0, 1)) * u;
    top + (bottom - top) * v
}

/// Fractal value noise: `octaves` layers of `value_noise`, each twice as fine and half as strong, 0..=1.
fn fbm(seed: u64, x: f32, y: f32, octaves: u64) -> f32 {
    let (mut sum, mut weight, mut amp) = (0.0, 0.0, 1.0);
    for o in 0..octaves {
        let f = (1u64 << o) as f32;
        sum += amp * value_noise(hash(seed, o), x * f, y * f);
        weight += amp;
        amp *= 0.5;
    }
    sum / weight
}

/// Sobel gradient magnitude of the luma, 0..=1, so a full-contrast step is 1.
fn edges(src: &Buf) -> Vec<f32> {
    let (w, h) = (src.w, src.h);
    let l: Vec<f32> = src.px.as_chunks::<4>().0.iter().map(|p| luma(p)).collect();
    let at = |sx: usize, sy: usize| l[sy * w + sx];
    let mut out = vec![0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let (yu, yd) = (y.saturating_sub(1), (y + 1).min(h - 1));
        for (x, o) in row.iter_mut().enumerate() {
            let (xl, xr) = (x.saturating_sub(1), (x + 1).min(w - 1));
            let gx = at(xr, yu) + 2.0 * at(xr, y) + at(xr, yd) - at(xl, yu) - 2.0 * at(xl, y) - at(xl, yd);
            let gy = at(xl, yd) + 2.0 * at(x, yd) + at(xr, yd) - at(xl, yu) - 2.0 * at(x, yu) - at(xr, yu);
            *o = (gx.hypot(gy) / 4.0).min(1.0);
        }
    });
    out
}

/// The colour at `t` along `stops`, which are sorted by position and run from 0 to 1.
fn ramp(t: f32, stops: &[(f32, [f32; 3])]) -> [f32; 3] {
    let t = unit(t);
    for pair in stops.windows(2) {
        let ((a, ca), (b, cb)) = (pair[0], pair[1]);
        if t <= b {
            let k = if b > a { (t - a) / (b - a) } else { 1.0 };
            return lerp3(ca, cb, k.clamp(0.0, 1.0));
        }
    }
    stops.last().map_or([0.0; 3], |s| s.1)
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalise(v: [f32; 3]) -> [f32; 3] {
    let len = dot(v, v).sqrt();
    v.map(|c| c / len)
}

/// A straight pixel as premultiplied floats, with `weight` applied on top of its own alpha.
fn layer(p: [u8; 4], weight: f32) -> [f32; 4] {
    let a = f32::from(p[3]) / 255.0 * weight;
    [f32::from(p[0]) / 255.0 * a, f32::from(p[1]) / 255.0 * a, f32::from(p[2]) / 255.0 * a, a]
}

/// Premultiplied `top` over `below`.
fn over(top: [f32; 4], below: [f32; 4]) -> [f32; 4] {
    let t = 1.0 - top[3];
    std::array::from_fn(|c| top[c] + below[c] * t)
}

/// Straight RGBA8 from premultiplied floats in 0..=1.
fn straight(pm: [f32; 4]) -> [u8; 4] {
    let a = pm[3].clamp(0.0, 1.0);
    if a <= 0.0 {
        return [0; 4];
    }
    [to_u8(pm[0] / a * 255.0), to_u8(pm[1] / a * 255.0), to_u8(pm[2] / a * 255.0), to_u8(a * 255.0)]
}

/// Stamps `shape` (given the offset from `centre`) into the coverage map out to `reach` px. Overlapping
/// shapes keep the larger coverage.
fn stamp(cover: &mut [f32], w: usize, h: usize, centre: [f32; 2], reach: f32, shape: impl Fn(f32, f32) -> f32) {
    let x0 = (centre[0] - reach).floor().max(0.0) as usize;
    let x1 = (centre[0] + reach).ceil().clamp(0.0, w as f32 - 1.0) as usize;
    let y0 = (centre[1] - reach).floor().max(0.0) as usize;
    let y1 = (centre[1] + reach).ceil().clamp(0.0, h as f32 - 1.0) as usize;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let v = shape(x as f32 + 0.5 - centre[0], y as f32 + 0.5 - centre[1]);
            let i = y * w + x;
            cover[i] = cover[i].max(v);
        }
    }
}

/// Alpha-weighted colour sums per cell, with `cell_of` giving each pixel's cell: red, green, blue, the
/// alpha sum and the pixel count.
fn cell_sums(src: &Buf, count: usize, cell_of: impl Fn(usize, usize) -> usize) -> Vec<[f32; 5]> {
    let mut acc = vec![[0f32; 5]; count];
    for y in 0..src.h {
        for x in 0..src.w {
            let p = px_at(src, x, y);
            let a = f32::from(p[3]);
            let c = &mut acc[cell_of(x, y)];
            c[0] += f32::from(p[0]) * a;
            c[1] += f32::from(p[1]) * a;
            c[2] += f32::from(p[2]) * a;
            c[3] += a;
            c[4] += 1.0;
        }
    }
    acc
}

/// The average colour of one cell's sums, transparent when the cell has no alpha.
fn cell_colour(c: [f32; 5]) -> [u8; 4] {
    if c[3] <= 0.0 {
        return [0; 4];
    }
    pack([c[0] / c[3], c[1] / c[3], c[2] / c[3]], to_u8(c[3] / c[4]))
}

/// Separable 8-point DCT basis: `t[u][x]` is the orthonormal cosine of frequency `u` at sample `x`.
fn dct_table() -> [[f32; BLOCK]; BLOCK] {
    std::array::from_fn(|u| {
        let scale = if u == 0 { (1.0f32 / 8.0).sqrt() } else { (2.0f32 / 8.0).sqrt() };
        std::array::from_fn(|x| scale * (((2 * x + 1) * u) as f32 * PI / 16.0).cos())
    })
}

/// Forward 8x8 DCT of a block in row-major order, rows then columns.
fn dct(block: &[f32; 64], t: &[[f32; BLOCK]; BLOCK]) -> [f32; 64] {
    let mut tmp = [0f32; 64];
    for y in 0..BLOCK {
        for u in 0..BLOCK {
            tmp[y * BLOCK + u] = (0..BLOCK).map(|x| block[y * BLOCK + x] * t[u][x]).sum::<f32>();
        }
    }
    let mut out = [0f32; 64];
    for v in 0..BLOCK {
        for u in 0..BLOCK {
            out[v * BLOCK + u] = (0..BLOCK).map(|y| tmp[y * BLOCK + u] * t[v][y]).sum::<f32>();
        }
    }
    out
}

/// Inverse 8x8 DCT, the reverse of `dct`.
fn idct(coef: &[f32; 64], t: &[[f32; BLOCK]; BLOCK]) -> [f32; 64] {
    let mut tmp = [0f32; 64];
    for y in 0..BLOCK {
        for u in 0..BLOCK {
            tmp[y * BLOCK + u] = (0..BLOCK).map(|v| coef[v * BLOCK + u] * t[v][y]).sum::<f32>();
        }
    }
    let mut out = [0f32; 64];
    for y in 0..BLOCK {
        for x in 0..BLOCK {
            out[y * BLOCK + x] = (0..BLOCK).map(|u| tmp[y * BLOCK + u] * t[u][x]).sum::<f32>();
        }
    }
    out
}

// ---- Kernels ----

/// Film strip frame: a `strip`-coloured band `width` px deep, with sprocket holes in `hole` down the left
/// and right margins. The canvas grows by `width` on every side.
pub(super) fn film_strip_frame(src: &Buf, width: u32, strip: Rgba, hole: Rgba) -> Result<Buf, EffectError> {
    let m = width as usize;
    let (cw, ch) = (src.w + 2 * m, src.h + 2 * m);
    let period = (2 * m).max(4);
    let (strip, hole) = (rgba4(strip), rgba4(hole));
    frame(src, width, |x, y, _| {
        let side = x.min(cw - 1 - x);
        let beside_image = (m..ch - m).contains(&y);
        let phase = y % period;
        let in_hole = beside_image && (m / 4..m - m / 4).contains(&side) && (period / 4..period - period / 4).contains(&phase);
        if in_hole { hole } else { strip }
    })
}

/// Frosted glass: each pixel averages `SCATTER` samples taken at random points within `radius` px, then
/// clouds toward white by `opacity` with a little grain. Alpha is kept.
pub(super) fn frosted_glass(src: &Buf, radius: u32, opacity: f32, seed: u64) -> Buf {
    frost(src, radius, opacity, seed)
}

fn frost(src: &Buf, radius: u32, opacity: f32, seed: u64) -> Buf {
    let k = unit(opacity);
    let span = 2.0 * radius.min(MAX_FROST_RADIUS) as f32;
    let w = src.w;
    per_pixel(src, |x, y| {
        let i = (y * w + x) as u64;
        let mut acc = [0f32; 3];
        for s in 0..SCATTER {
            let slot = i * 2 * SCATTER + 2 * s;
            let dx = (unit_hash(seed, slot) - 0.5) * span;
            let dy = (unit_hash(seed, slot + 1) - 0.5) * span;
            let q = sample(src, [x as f32 + 0.5 + dx, y as f32 + 0.5 + dy]);
            for (a, v) in acc.iter_mut().zip(rgb3(q)) {
                *a += v;
            }
        }
        let base = acc.map(|a| a / SCATTER as f32);
        let grain = (unit_hash(seed ^ FROST_SALT, i) - 0.5) * 24.0 * k;
        let p = px_at(src, x, y);
        pack(lerp3(base, [255.0; 3], 0.35 * k).map(|v| v + grain), p[3])
    })
}

/// Frosted glass with ice at the edges: the frost of `frosted_glass` and, within `edge` px of the border,
/// crystals of `ice` that thicken toward the outside.
pub(super) fn frosted_glass_ice_edges(src: &Buf, radius: u32, edge: u32, ice: Rgba, seed: u64) -> Buf {
    let frosted = frost(src, radius, 0.5, seed);
    let (w, h) = (src.w, src.h);
    let edge = edge.min(MAX_EDGE) as f32;
    let ice = rgb_of(ice);
    per_pixel(&frosted, |x, y| {
        let p = px_at(&frosted, x, y);
        let d = x.min(y).min(w - 1 - x).min(h - 1 - y) as f32 + 0.5;
        let inward = 1.0 - smoothstep(0.0, edge, d);
        let crystal = smoothstep(0.5, 0.7, value_noise(seed ^ EDGE_SALT, x as f32 / 2.5, y as f32 / 2.5));
        let k = (inward * (0.3 + 0.7 * crystal)).clamp(0.0, 1.0) * 0.9;
        pack(lerp3(rgb3(p), ice, k), p[3])
    })
}

/// Frost ice border: a `base` frame where `ice` crystals grow in from the outer edge. Each facet of four px
/// along the edge reaches its own depth, so the tips are jagged. The canvas grows by `width` on every side.
pub(super) fn frost_ice_border(src: &Buf, width: u32, ice: Rgba, base: Rgba, seed: u64) -> Result<Buf, EffectError> {
    let m = width as usize;
    let (cw, ch) = (src.w + 2 * m, src.h + 2 * m);
    let (ice, base) = (rgb_of(ice), rgb_of(base));
    let band = m as f32;
    frame(src, width, |x, y, depth| {
        // Across the top and bottom the crystals run along x; across the sides, along y.
        let along = if y.min(ch - 1 - y) <= x.min(cw - 1 - x) { x } else { y };
        let reach = band * (0.35 + 0.65 * unit_hash(seed, (along / 4) as u64));
        let d = depth as f32 + 0.5;
        if d < reach { pack(lerp3(ice, [255.0; 3], 0.5 * (d / reach).clamp(0.0, 1.0)), 255) } else { pack(base, 255) }
    })
}

/// Ghost trail: `copies` faded copies of the image, each shifted a further `offset` px, sit behind it. The
/// nearest copy is `opacity` strong and each farther one fades out.
pub(super) fn ghost_trail(src: &Buf, copies: u32, offset: (i32, i32), opacity: f32) -> Buf {
    let copies = i64::from(copies.min(MAX_GHOSTS));
    let k = unit(opacity);
    let (ox, oy) = (i64::from(offset.0), i64::from(offset.1));
    per_pixel(src, |x, y| {
        let (xi, yi) = (x as i64, y as i64);
        // Farthest copy first, so each nearer one lands on top of it.
        let mut behind = [0f32; 4];
        for step in (1..=copies).rev() {
            let weight = k * (1.0 - step as f32 / (copies as f32 + 1.0));
            let q = at_or_clear(src, xi - step * ox, yi - step * oy);
            behind = over(layer(q, weight), behind);
        }
        straight(over(layer(px_at(src, x, y), 1.0), behind))
    })
}

/// Glare: pixels brighter than `threshold` (0..=1 luma) throw light along eight rays, `length` px out
/// (capped at 48) and fading with distance. The light is added in white at `strength`.
pub(super) fn glare(src: &Buf, threshold: f32, length: u32, strength: f32) -> Buf {
    const RAYS: [(i64, i64); 8] = [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (-1, -1), (1, -1), (-1, 1)];
    let threshold = unit(threshold).min(0.999);
    let reach = i64::from(length.min(MAX_GLARE));
    let gain = cap(strength, 4.0).max(0.0) * 0.25 / reach.max(1) as f32;
    let (w, h) = (src.w, src.h);
    let bright: Vec<f32> = src
        .px
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| ((luma(p) - threshold) / (1.0 - threshold)).clamp(0.0, 1.0) * f32::from(p[3]) / 255.0)
        .collect();
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let mut glow = 0.0f32;
        for (dx, dy) in RAYS {
            for s in 1..=reach {
                let (sx, sy) = (x as i64 + dx * s, y as i64 + dy * s);
                if !(0..w as i64).contains(&sx) || !(0..h as i64).contains(&sy) {
                    break;
                }
                let t = 1.0 - s as f32 / (reach as f32 + 1.0);
                glow += bright[sy as usize * w + sx as usize] * t * t;
            }
        }
        pack(rgb3(p).map(|v| v + 255.0 * gain * glow), p[3])
    })
}

/// Glitch shift: `bands` random horizontal bands, each shifted up to `max_shift` px sideways. Red and blue
/// slide a little further out and in than green, so the bands fringe.
pub(super) fn glitch_shift(src: &Buf, bands: u32, max_shift: u32, seed: u64) -> Buf {
    if bands == 0 || max_shift == 0 {
        return src.clone();
    }
    let (w, h) = (src.w, src.h);
    let h32 = h as u32;
    let mut shifts = vec![[0i64; 3]; h];
    let mut rng = Rng::new(seed ^ GLITCH_SALT);
    for _ in 0..bands.min(MAX_GLITCH_BANDS) {
        let band_h = rng.range(1, (h32 / 8).max(1)) as usize;
        let top = rng.range(0, h32 - 1) as usize;
        let sign: i64 = if rng.next_u64() & 1 == 0 { 1 } else { -1 };
        let shift = i64::from(rng.range(1, max_shift)) * sign;
        let spread = i64::from(rng.range(0, max_shift / 2));
        for row in shifts.iter_mut().skip(top).take(band_h) {
            *row = [shift + spread, shift, shift - spread];
        }
    }
    per_pixel(src, |x, y| {
        let [dr, dg, db] = shifts[y];
        let r = px_at(src, shifted(x, dr, w), y);
        let g = px_at(src, shifted(x, dg, w), y);
        let b = px_at(src, shifted(x, db, w), y);
        [r[0], g[1], b[2], px_at(src, x, y)[3]]
    })
}

/// Golden border: a `color` frame with dark rules at its outer and inner edges and a bright sheen a third
/// of the way in. The canvas grows by `width` on every side.
pub(super) fn golden_border(src: &Buf, width: u32, color: Rgba) -> Result<Buf, EffectError> {
    let m = width as usize;
    frame(src, width, |_, _, depth| {
        let t = (depth as f32 + 0.5) / m as f32;
        let rule = depth == 0 || depth + 1 == m;
        let k = if rule { 0.45 } else { 0.55 + 0.45 * (PI * t).sin() + 0.35 * (-((t - 0.3) / 0.1).powi(2)).exp() };
        shade(color, k)
    })
}

/// Gradient fade border: the margin fades from `color` at the outer edge into the image pixel beside it.
/// The canvas grows by `width` on every side.
pub(super) fn gradient_fade_border(src: &Buf, width: u32, color: Rgba) -> Result<Buf, EffectError> {
    let m = width as i64;
    let (w, h) = (src.w as i64, src.h as i64);
    let ink = rgb_of(color);
    frame(src, width, |x, y, depth| {
        let ix = (x as i64 - m).clamp(0, w - 1) as usize;
        let iy = (y as i64 - m).clamp(0, h - 1) as usize;
        let beside = rgb3(px_at(src, ix, iy));
        let t = smoothstep(0.0, 1.0, (depth as f32 + 0.5) / m as f32);
        pack(lerp3(ink, beside, t), 255)
    })
}

/// Halation: the light from bright areas spills into a red-orange halo, `radius` px wide, added at `strength`.
pub(super) fn halation(src: &Buf, threshold: f32, radius: u32, strength: f32) -> Buf {
    let threshold = unit(threshold).min(0.999);
    let gain = cap(strength, 4.0).max(0.0) * 255.0;
    let (w, h) = (src.w, src.h);
    let mut halo: Vec<f32> =
        src.px.as_chunks::<4>().0.iter().map(|p| smoothstep(threshold, 1.0, luma(p)) * f32::from(p[3]) / 255.0).collect();
    blur::box3(&mut halo, w, h, 1, radius.min(MAX_HALO_RADIUS) as usize);
    // Film emulsion reacts most to red and least to blue, so the halo leans orange.
    const TINT: [f32; 3] = [1.0, 0.35, 0.15];
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let k = halo[y * w + x] * gain;
        pack(std::array::from_fn(|c| f32::from(p[c]) + k * TINT[c]), p[3])
    })
}

/// Heat haze: rows of air wobble sideways. Each row shifts by a sine of its height, `amplitude` px at most
/// (capped at 64), with a wavelength of `wavelength` px (at least 2).
pub(super) fn heat_haze(src: &Buf, amplitude: u32, wavelength: u32) -> Buf {
    let amp = amplitude.min(MAX_HAZE) as f32;
    let period = wavelength.max(2) as f32;
    per_pixel(src, |x, y| {
        let fy = y as f32 + 0.5;
        // A slower swell modulates the wobble so the rows don't all move in step.
        let swell = 0.7 + 0.3 * (TAU * fy / (period * 3.1) + 1.0).sin();
        let dx = amp * swell * (TAU * fy / period).sin();
        sample(src, [x as f32 + 0.5 + dx, fy])
    })
}

/// Black through indigo, magenta and red to yellow and white, by luma.
const HEAT: [(f32, [f32; 3]); 6] = [
    (0.0, [0.0, 0.0, 0.0]),
    (0.2, [40.0, 0.0, 90.0]),
    (0.4, [180.0, 0.0, 120.0]),
    (0.6, [230.0, 40.0, 20.0]),
    (0.8, [255.0, 190.0, 30.0]),
    (1.0, [255.0, 255.0, 240.0]),
];

/// Heat map: each pixel's luma picks a colour from the thermal ramp. Alpha is kept.
pub(super) fn heat_map(src: &Buf) -> Buf {
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        pack(ramp(luma(&p), &HEAT), p[3])
    })
}

/// The axial cell of pixel (x, y) for flat-top hexagons of circumradius `s`, rounded in cube coordinates.
fn hex_cell(x: usize, y: usize, s: f32) -> (i64, i64) {
    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
    let q = 2.0 / 3.0 * px / s;
    let r = (-px / 3.0 + SQRT_3 / 3.0 * py) / s;
    let (xc, yc, zc) = (q, -q - r, r);
    let (rx, ry, rz) = (xc.round(), yc.round(), zc.round());
    let (dx, dy, dz) = ((rx - xc).abs(), (ry - yc).abs(), (rz - zc).abs());
    let (qc, rc) = if dx > dy && dx > dz {
        (-ry - rz, rz)
    } else if dy > dz {
        (rx, rz)
    } else {
        (rx, -rx - ry)
    };
    (qc as i64, rc as i64)
}

/// Hexagonal pixelate: each flat-top hexagon of circumradius `size` px takes the average colour of the
/// pixels in it. Hexagons are enlarged for very large images so the grid stays at most `MAX_HEX_GRID` across.
pub(super) fn hexagonal_pixelate(src: &Buf, size: u32) -> Buf {
    let (w, h) = (src.w, src.h);
    let s = (size.clamp(1, MAX_HEX_SIZE) as f32).max((w + h) as f32 / MAX_HEX_GRID);
    // Bounds of the axial cells the image touches, with a margin for rounding.
    let (q0, q1) = (-2i64, (2.0 * w as f32 / (3.0 * s)).ceil() as i64 + 2);
    let (r0, r1) = ((-(w as f32) / (3.0 * s)).floor() as i64 - 2, (SQRT_3 / 3.0 * h as f32 / s).ceil() as i64 + 2);
    let (nq, nr) = ((q1 - q0 + 1) as usize, (r1 - r0 + 1) as usize);
    let index = |x: usize, y: usize| {
        let (q, r) = hex_cell(x, y, s);
        let qi = (q - q0).clamp(0, nq as i64 - 1) as usize;
        let ri = (r - r0).clamp(0, nr as i64 - 1) as usize;
        qi * nr + ri
    };
    let sums = cell_sums(src, nq * nr, index);
    per_pixel(src, |x, y| cell_colour(sums[index(x, y)]))
}

/// Hologram scan: the image turns to a `tint` monochrome by `strength`, with dark scanlines every
/// `line_spacing` rows (1..=64) and a bright roll band near the lower part of the image.
pub(super) fn hologram_scan(src: &Buf, line_spacing: u32, strength: f32, tint: Rgba) -> Buf {
    let k = unit(strength);
    let spacing = line_spacing.clamp(1, MAX_SCAN_SPACING) as usize;
    let tint = rgb_of(tint);
    let h = src.h as f32;
    let (roll, roll_w) = (0.7 * h, (0.05 * h).max(1.0));
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let mono = tint.map(|c| c * (0.2 + 0.8 * luma(&p)));
        let fy = y as f32 + 0.5;
        let scan = if y.is_multiple_of(spacing) { 1.0 - 0.6 * k } else { 1.0 };
        let bump = 1.0 + 0.6 * k * (-((fy - roll) / roll_w).powi(2)).exp();
        let flicker = 1.0 + 0.1 * k * (unit_hash(SCAN_SALT, y as u64) - 0.5);
        let col = lerp3(rgb3(p), mono, k);
        pack(col.map(|c| c * scan * bump * flicker), p[3])
    })
}

/// Holographic foil shimmer: diagonal bands of rainbow colour, `period` px per cycle (1..=4096), multiplied
/// into the image at `strength`. The bands drift with luma, so bright areas shift hue.
pub(super) fn holographic_foil(src: &Buf, strength: f32, period: u32) -> Buf {
    let k = unit(strength);
    let period = period.clamp(1, MAX_SHIMMER_PERIOD) as f32;
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let t = (x + y) as f32 / period + 0.5 * luma(&p);
        // Each channel swings through its own third of a cosine rainbow.
        let hue: [f32; 3] = std::array::from_fn(|c| 0.5 + 0.5 * (TAU * (t + c as f32 / 3.0)).cos());
        let base = rgb3(p);
        pack(std::array::from_fn(|c| base[c] * ((1.0 - k) + 2.0 * k * hue[c])), p[3])
    })
}

/// Infrared false colour: near-infrared stands in for green foliage, so leaves turn red, and the blue of the
/// sky shifts toward purple. `strength` mixes from the original to the false colour.
pub(super) fn infrared_false_color(src: &Buf, strength: f32) -> Buf {
    let k = unit(strength);
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let base = rgb3(p);
        let nir = 0.8 * base[1] + 0.2 * base[0];
        let false_colour = [nir, base[0], base[2] * 0.8];
        pack(lerp3(base, false_colour, k), p[3])
    })
}

/// Ink splatter: `count` splats of ink `color`, each a wobbly blob `size` (0..=1 of the shorter side)
/// across, with satellite droplets flung out around it and a couple of drips running down.
pub(super) fn ink_splatter(src: &Buf, count: u32, size: f32, color: Rgba, seed: u64) -> Buf {
    let size = unit(size);
    if count == 0 || size <= 0.0 {
        return src.clone();
    }
    let (w, h) = (src.w, src.h);
    let (wf, hf) = (w as f32, h as f32);
    let base = wf.min(hf) * size;
    let mut cover = vec![0f32; w * h];
    for k in 0..u64::from(count.min(MAX_INK_SPLATS)) {
        // Slots 0 to 63 belong to one splat, so the splats don't share numbers.
        let key = |slot: u64| unit_hash(seed ^ INK_SALT, 64 * k + slot);
        let centre = [key(0) * wf, key(1) * hf];
        let r = base * (0.5 + 0.5 * key(2));
        let phase = key(3) * TAU;
        stamp(&mut cover, w, h, centre, r * 1.3, |dx, dy| {
            let theta = dy.atan2(dx);
            let rim = r * (0.82 + 0.12 * (3.0 * theta + phase).sin() + 0.06 * (7.0 * theta - 2.0 * phase).sin());
            smoothstep(rim + 0.5, rim - 0.5, dx.hypot(dy))
        });
        for d in 0..INK_DROPS {
            let slot = 4 + 2 * d as u64;
            let (angle, far) = (key(slot) * TAU, key(slot + 1));
            let drop_r = (r * 0.08 * (0.4 + far)).max(0.8);
            let dist = r * (1.0 + 2.2 * far * far);
            let drop_centre = [centre[0] + dist * angle.cos(), centre[1] + dist * angle.sin()];
            stamp(&mut cover, w, h, drop_centre, drop_r + 1.0, |dx, dy| smoothstep(drop_r + 0.5, drop_r - 0.5, dx.hypot(dy)));
        }
        for j in 0..2u64 {
            let slot = 52 + 3 * j;
            let x_off = (key(slot) - 0.5) * r;
            let len = r * (1.5 + 2.5 * key(slot + 1));
            let half_w = (r * 0.06 * (0.5 + key(slot + 2))).max(0.6);
            let mid = [centre[0] + x_off, centre[1] + len / 2.0];
            stamp(&mut cover, w, h, mid, len / 2.0 + half_w + 1.0, |dx, dy| {
                smoothstep(half_w + 0.5, half_w - 0.5, dx.abs()) * smoothstep(len / 2.0 + 0.5, len / 2.0 - 0.5, dy.abs())
            });
        }
    }
    let ink = rgb_of(color);
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let c = cover[y * w + x].clamp(0.0, 1.0);
        pack(lerp3(rgb3(p), ink, c), p[3])
    })
}

/// Inner shadow: the inside edge of the shape darkens to `color`, as if the shape were cut out and lit from
/// the top left. `offset` moves the light; `size` is the blur in px (capped at 256).
pub(super) fn inner_shadow(src: &Buf, opacity: f32, size: u32, color: Rgba, offset: (i32, i32)) -> Buf {
    let k = unit(opacity);
    let pad = size.min(MAX_SHADOW_REACH) as usize;
    let (w, h) = (src.w, src.h);
    let (pw, ph) = (w + 2 * pad, h + 2 * pad);
    let (ox, oy) = (i64::from(offset.0), i64::from(offset.1));
    // Outside the shape is transparent, so the mask is 1 there: the shadow falls along the canvas edge.
    let mut mask = vec![0f32; pw * ph];
    for (py, row) in mask.chunks_mut(pw).enumerate() {
        for (col, m) in row.iter_mut().enumerate() {
            let ix = col as i64 - pad as i64 - ox;
            let iy = py as i64 - pad as i64 - oy;
            let a = if (0..w as i64).contains(&ix) && (0..h as i64).contains(&iy) {
                f32::from(src.px[(iy as usize * w + ix as usize) * 4 + 3]) / 255.0
            } else {
                0.0
            };
            *m = 1.0 - a;
        }
    }
    blur::box3(&mut mask, pw, ph, 1, pad);
    let ink = rgb_of(color);
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let s = mask[(y + pad) * pw + x + pad] * f32::from(p[3]) / 255.0 * k;
        pack(lerp3(rgb3(p), ink, s), p[3])
    })
}

/// Colour to YCbCr with luma shifted to zero-centred, as the JPEG transform works on signed values.
fn to_ycc(p: [u8; 4]) -> [f32; 3] {
    let [r, g, b] = rgb3(p);
    [0.299 * r + 0.587 * g + 0.114 * b - 128.0, -0.168_736 * r - 0.331_264 * g + 0.5 * b, 0.5 * r - 0.418_688 * g - 0.081_312 * b]
}

/// JPEG artifact: each 8x8 block of luma and chroma goes through a DCT, its coefficients are rounded to
/// steps that grow with spatial frequency, and it goes back. `quality` 100 is near lossless; lower values
/// block up and ring. Chroma steps are twice as coarse. Alpha is kept.
pub(super) fn jpeg_artifact(src: &Buf, quality: u8) -> Buf {
    let quality = f32::from(quality.clamp(1, 100));
    let scale = (100.0 - quality) / 10.0;
    let (w, h) = (src.w, src.h);
    let table = dct_table();
    let mut steps = [[0f32; 64]; 2];
    for v in 0..BLOCK {
        for u in 0..BLOCK {
            let s = 1.0 + scale * (1 + u + v) as f32;
            steps[0][v * BLOCK + u] = s;
            steps[1][v * BLOCK + u] = 2.0 * s;
        }
    }
    let ycc: Vec<[f32; 3]> = src.px.as_chunks::<4>().0.par_iter().map(|p| to_ycc(*p)).collect();
    let mut out = vec![[0f32; 3]; w * h];
    out.par_chunks_mut(BLOCK * w).enumerate().for_each(|(by, rows)| {
        let band = rows.len() / w;
        for bx in 0..w.div_ceil(BLOCK) {
            let mut recon = [[0f32; 64]; 3];
            for (c, plane) in recon.iter_mut().enumerate() {
                let mut block = [0f32; 64];
                for j in 0..BLOCK {
                    for i in 0..BLOCK {
                        let sx = (bx * BLOCK + i).min(w - 1);
                        let sy = (by * BLOCK + j).min(h - 1);
                        block[j * BLOCK + i] = ycc[sy * w + sx][c];
                    }
                }
                let mut coef = dct(&block, &table);
                let step = &steps[usize::from(c > 0)];
                for (v, s) in coef.iter_mut().zip(step) {
                    *v = (*v / s).round() * s;
                }
                *plane = idct(&coef, &table);
            }
            for j in 0..band {
                for i in 0..BLOCK {
                    let x = bx * BLOCK + i;
                    if x < w {
                        let k = j * BLOCK + i;
                        rows[j * w + x] = [recon[0][k], recon[1][k], recon[2][k]];
                    }
                }
            }
        }
    });
    per_pixel(src, |x, y| {
        let [yy, cb, cr] = out[y * w + x];
        let p = px_at(src, x, y);
        let r = yy + 128.0 + 1.402 * cr;
        let g = yy + 128.0 - 0.344_136 * cb - 0.714_136 * cr;
        let b = yy + 128.0 + 1.772 * cb;
        pack([r, g, b], p[3])
    })
}

/// Lava molten border: a frame of crust and molten rock, with cracks of `hot` glowing where a noise ridge
/// runs. `cool` sets the crust. The canvas grows by `width` on every side.
pub(super) fn lava_border(src: &Buf, width: u32, hot: Rgba, cool: Rgba, seed: u64) -> Result<Buf, EffectError> {
    let hot = rgb_of(hot);
    let crust = rgb_of(cool).map(|c| c * 0.35);
    frame(src, width, |x, y, _| {
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let n = 0.7 * value_noise(seed, fx / 9.0, fy / 9.0) + 0.3 * value_noise(seed ^ LAVA_SALT, fx / 3.0, fy / 3.0);
        // Cracks sit where the noise crosses its middle, so the ridge is brightest there.
        let ridge = 1.0 - (2.0 * n - 1.0).abs();
        let crack = smoothstep(0.8, 0.95, ridge);
        let molten = lerp3(crust, hot, crack.max(0.25 * n));
        pack(lerp3(molten, [255.0, 235.0, 180.0], 0.6 * smoothstep(0.93, 1.0, ridge)), 255)
    })
}

/// Lens blur: each pixel averages `radius` px of its surroundings over a disc, sampled on a spiral. Bright
/// taps count for more (`highlights`, up to 16), so highlights bloom into discs.
pub(super) fn lens_blur(src: &Buf, radius: u32, highlights: f32) -> Buf {
    let r = radius.min(MAX_LENS_RADIUS) as f32;
    if r == 0.0 {
        return src.clone();
    }
    let boost = cap(highlights, 16.0).max(0.0);
    per_pixel(src, |x, y| {
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let (mut rgb, mut alpha, mut weight) = ([0f32; 3], 0f32, 0f32);
        for k in 0..LENS_TAPS {
            let t = ((k as f32 + 0.5) / LENS_TAPS as f32).sqrt() * r;
            let (sin, cos) = (k as f32 * GOLDEN_ANGLE).sin_cos();
            let q = sample(src, [fx + t * cos, fy + t * sin]);
            let a = f32::from(q[3]) / 255.0;
            let ws = 1.0 + boost * luma(&q).powi(4);
            for (acc, v) in rgb.iter_mut().zip(rgb3(q)) {
                *acc += v * a * ws;
            }
            alpha += a * ws;
            weight += ws;
        }
        if alpha <= 0.0 {
            return [0; 4];
        }
        pack(rgb.map(|v| v / alpha), to_u8(alpha / weight * 255.0))
    })
}

/// Light leak: a `color` glow from a point (`center`, fractions of the image) that falls off over `radius`
/// (a fraction of the image diagonal), screened over the image at `strength`.
pub(super) fn light_leak(src: &Buf, center: (f32, f32), radius: f32, strength: f32, color: Rgba) -> Buf {
    let (w, h) = (src.w as f32, src.h as f32);
    let (cx, cy) = (unit(center.0) * w, unit(center.1) * h);
    let reach = w.hypot(h) * unit(radius).max(0.01);
    let k = unit(strength);
    let tint = rgb_of(color).map(|v| v / 255.0);
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let d = (x as f32 + 0.5 - cx).hypot(y as f32 + 0.5 - cy) / reach;
        let g = (1.0 - d).clamp(0.0, 1.0).powi(2) * k;
        pack(std::array::from_fn(|c| screen(p[c], tint[c] * g)), p[3])
    })
}

/// Liquid glass: the image seen through a wavy pane. Each pixel is pulled by up to `strength` px along two
/// smooth noise fields (`scale` px across), and the ridges of the field catch the light.
pub(super) fn liquid_glass(src: &Buf, strength: u32, scale: u32, seed: u64) -> Buf {
    let amp = strength.min(MAX_GLASS) as f32;
    let s = scale.clamp(2, MAX_CELL) as f32;
    per_pixel(src, |x, y| {
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let hx = value_noise(seed, fx / s, fy / s);
        let hy = value_noise(seed ^ GLASS_SALT, fx / s, fy / s);
        let q = sample(src, [fx + amp * (2.0 * hx - 1.0), fy + amp * (2.0 * hy - 1.0)]);
        let ridge = smoothstep(0.85, 1.0, 1.0 - (2.0 * hx - 1.0).abs());
        pack(lerp3(rgb3(q), [255.0; 3], 0.35 * ridge), q[3])
    })
}

/// Liquid mercury: the image as a silver liquid. Its height is smooth noise plus the image's luma, and the
/// surface is lit from the top left, so highlights run as bright ridges. `scale` sets the ripples in px.
pub(super) fn liquid_mercury(src: &Buf, scale: u32, seed: u64) -> Buf {
    let s = scale.clamp(2, MAX_CELL) as f32;
    let height = |x: f32, y: f32| 0.6 * value_noise(seed, x / s, y / s) + 0.4 * luma(&sample(src, [x, y]));
    let light = normalise([-0.5, -0.6, 0.9]);
    let half = normalise([light[0], light[1], light[2] + 1.0]);
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let gx = (height(fx + 1.0, fy) - height(fx - 1.0, fy)) * 6.0;
        let gy = (height(fx, fy + 1.0) - height(fx, fy - 1.0)) * 6.0;
        let n = normalise([-gx, -gy, 1.0]);
        let diffuse = dot(n, light).max(0.0);
        let spec = dot(n, half).max(0.0).powi(40);
        let v = 255.0 * (0.12 + 0.7 * diffuse + 1.1 * spec).min(1.0);
        pack([v * 0.96, v * 0.98, v], p[3])
    })
}

/// Luminance contour lines: the image is washed toward white, and `color` lines are drawn wherever luma
/// crosses one of `levels` bands (2..=64).
pub(super) fn contour_lines(src: &Buf, levels: u8, color: Rgba) -> Buf {
    let n = f32::from(levels.clamp(2, 64));
    let (w, h) = (src.w, src.h);
    let band = |x: usize, y: usize| (luma(&px_at(src, x, y)) * n).floor().min(n - 1.0);
    let line = rgb_of(color);
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let here = band(x, y);
        let right = band((x + 1).min(w - 1), y);
        let below = band(x, (y + 1).min(h - 1));
        let on_line = (here - right).abs() > 0.5 || (here - below).abs() > 0.5;
        let ground = lerp3(rgb3(p), [255.0; 3], 0.8);
        pack(if on_line { line } else { ground }, p[3])
    })
}

/// Traffic-light colours of the title bar buttons: red, yellow, green.
const BUTTONS: [[f32; 3]; 3] = [[255.0, 95.0, 86.0], [255.0, 189.0, 46.0], [39.0, 201.0, 63.0]];

/// macOS window: a `bar_color` title bar `bar` px tall above the image, with three buttons, and rounded
/// corners `radius` px across on all four corners of the canvas. The canvas grows by `bar` at the top.
pub(super) fn macos_window(src: &Buf, bar: u32, radius: u32, bar_color: Rgba) -> Result<Buf, EffectError> {
    let bar_px = bar as usize;
    let (cw, ch) = fit(src.w as u64, src.h as u64 + u64::from(bar))?;
    let (cwf, chf) = (cw as f32, ch as f32);
    let r = (radius as f32).min(cwf / 2.0).min(chf / 2.0);
    let bar_f = bar as f32;
    let bar_c = rgb_of(bar_color);
    let button_r = (0.2 * bar_f).max(1.0);
    let mut px = vec![0u8; cw * ch * 4];
    px.par_chunks_mut(cw * 4).enumerate().for_each(|(y, row)| {
        for (x, out) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            // The nearest point of the rounded rectangle's core; outside the radius around it is transparent.
            let (qx, qy) = (fx.clamp(r, cwf - r), fy.clamp(r, chf - r));
            let inside = (fx - qx).hypot(fy - qy) <= r;
            *out = if !inside {
                [0; 4]
            } else if y < bar_px {
                let button = (0..3).find(|&i| (fx - bar_f * (0.6 + 0.6 * i as f32)).hypot(fy - bar_f / 2.0) < button_r);
                let colour = button.map_or(bar_c, |i| BUTTONS[i]);
                let rule = y + 1 == bar_px;
                pack(if rule { bar_c.map(|c| c * 0.7) } else { colour }, 255)
            } else {
                px_at(src, x, y - bar_px)
            };
        }
    });
    Ok(Buf { w: cw, h: ch, px })
}

/// Marble border: a `base` frame with veins of `vein`. Turbulence bends a diagonal set of stripes, and a vein
/// is where a stripe's sine crosses zero. The canvas grows by `width` on every side.
pub(super) fn marble_border(src: &Buf, width: u32, base: Rgba, vein: Rgba, seed: u64) -> Result<Buf, EffectError> {
    let (base, vein) = (rgb_of(base), rgb_of(vein));
    frame(src, width, |x, y, _| {
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let turb = fbm(seed, fx / 24.0, fy / 24.0, 4);
        let t = 0.08 * fx + 0.05 * fy + 6.0 * turb;
        let s = (t * PI).sin().abs();
        let k = 1.0 - smoothstep(0.0, 0.12, s);
        pack(lerp3(base, vein, 0.9 * k), 255)
    })
}

/// Matrix digital rain: columns of `column` px glyph cells (3 to 64). Each column has a bright head falling
/// down the image with a trail above it. Glyphs are 3x3 blocks drawn from the seed. The image dims by
/// `strength` and the trails light up in `color`.
pub(super) fn matrix_rain(src: &Buf, column: u32, strength: f32, color: Rgba, seed: u64) -> Buf {
    let cell = column.clamp(3, MAX_RAIN_CELL) as usize;
    let k = unit(strength);
    let ink = rgb_of(color);
    let cols = src.w.div_ceil(cell);
    let rows = src.h.div_ceil(cell) as f32;
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let (cx, ry) = (x / cell, y / cell);
        // Each column's head starts above the image and falls past its bottom.
        let head = unit_hash(seed ^ RAIN_SALT, cx as u64) * (rows + 24.0) - 12.0;
        let trail = 6.0 + 18.0 * unit_hash(seed ^ RAIN_SALT ^ 1, cx as u64);
        let dist = head - ry as f32;
        let bright = if (0.0..trail).contains(&dist) { (1.0 - dist / trail).powi(2) } else { 0.0 };
        let (gx, gy) = ((x % cell) * 3 / cell, (y % cell) * 3 / cell);
        let glyph_id = ((ry * cols + cx) as u64) * 9 + (gy * 3 + gx) as u64;
        let glyph = if unit_hash(seed ^ GLYPH_SALT, glyph_id) < 0.5 { 1.0 } else { 0.25 };
        let lit = (bright * glyph * k).clamp(0.0, 1.0);
        let head_glow = if (0.0..1.0).contains(&dist) { 0.6 } else { 0.0 };
        let tint = lerp3(ink, [255.0; 3], head_glow);
        let dim = rgb3(p).map(|v| v * (1.0 - 0.7 * k));
        pack(lerp3(dim, tint, lit), p[3])
    })
}

/// Minecraft grass: the image is pixelated into `block` px blocks, with a row of grass blocks over it and a
/// row of dirt blocks under it. Each column of blocks is shaded a little differently. The canvas grows by one
/// block on the top and one on the bottom.
pub(super) fn minecraft_grass(src: &Buf, block: u32, seed: u64) -> Result<Buf, EffectError> {
    let b = block.clamp(1, MAX_BLOCK) as usize;
    let pixel = cells::pixelate(src, b as u32);
    let (w, h) = (src.w, src.h);
    let (cw, ch) = fit(w as u64, h as u64 + 2 * b as u64)?;
    let mut px = vec![0u8; cw * ch * 4];
    px.par_chunks_mut(cw * 4).enumerate().for_each(|(y, row)| {
        for (x, out) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let column = (x / b) as u64;
            let speck = unit_hash(seed ^ GRASS_SALT, (y * cw + x) as u64);
            *out = if y < b {
                let shade = 0.8 + 0.4 * unit_hash(seed ^ GRASS_SALT, column);
                let k = if speck < 0.1 { 0.6 } else { shade };
                pack([96.0 * k, 165.0 * k, 66.0 * k], 255)
            } else if y < b + h {
                px_at(&pixel, x, y - b)
            } else {
                let shade = 0.8 + 0.4 * unit_hash(seed ^ DIRT_SALT, column);
                let stone = speck > 0.95;
                let base = if stone { [120.0, 120.0, 120.0] } else { [134.0, 96.0, 58.0] };
                pack(base.map(|c| c * shade), 255)
            };
        }
    });
    Ok(Buf { w: cw, h: ch, px })
}

/// Miniature: a tilt-shift look. Rows far from the `focus` band (0..=1 of the height, centre 0.5) blur by
/// `radius` px, `band` (0..=1) is the share of the height that stays sharp, and the whole image gains
/// saturation by `saturation` (0..=2).
pub(super) fn miniature(src: &Buf, focus: f32, band: f32, radius: u32, saturation: f32) -> Buf {
    let focus = unit(focus);
    let half = unit(band) * 0.5;
    let sat = cap(saturation, 2.0).max(0.0);
    let soft = blur::box_blur(src, radius.min(MAX_MINI_BLUR));
    let h = src.h as f32;
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let q = px_at(&soft, x, y);
        let d = ((y as f32 + 0.5) / h - focus).abs();
        let t = smoothstep(half, half + 0.15, d);
        let mixed = lerp3(rgb3(p), rgb3(q), t);
        let grey = 0.299 * mixed[0] + 0.587 * mixed[1] + 0.114 * mixed[2];
        pack(mixed.map(|c| grey + (c - grey) * (1.0 + sat)), p[3])
    })
}

/// Mosaic polygons: the image split into irregular cells, one jittered point per `size` px grid square. Each
/// cell takes its average colour, and the seams between cells are `grout`.
pub(super) fn mosaic_polygon(src: &Buf, size: u32, grout: Rgba, seed: u64) -> Buf {
    let s = size.clamp(2, MAX_CELL) as usize;
    let sf = s as f32;
    let (w, h) = (src.w, src.h);
    let (gw, gh) = (w.div_ceil(s), h.div_ceil(s));
    let site = |i: usize, j: usize| -> [f32; 2] {
        let k = ((j * gw + i) as u64) * 2;
        [(i as f32 + 0.15 + 0.7 * unit_hash(seed, k)) * sf, (j as f32 + 0.15 + 0.7 * unit_hash(seed, k + 1)) * sf]
    };
    // The cell of the nearest site to pixel (x, y), and how much nearer that site is than the next one:
    // 0 where two cells meet, 1 well inside a cell.
    let locate = |x: usize, y: usize| -> (usize, f32) {
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let (i0, j0) = (x / s, y / s);
        let (mut best, mut second, mut id) = (f32::MAX, f32::MAX, 0usize);
        for j in j0.saturating_sub(1)..=(j0 + 1).min(gh - 1) {
            for i in i0.saturating_sub(1)..=(i0 + 1).min(gw - 1) {
                let p = site(i, j);
                let d = (p[0] - fx).hypot(p[1] - fy);
                if d < best {
                    second = best;
                    best = d;
                    id = j * gw + i;
                } else if d < second {
                    second = d;
                }
            }
        }
        (id, 1.0 - smoothstep(0.0, 1.5, second - best))
    };
    let sums = cell_sums(src, gw * gh, |x, y| locate(x, y).0);
    let seam = rgb_of(grout);
    per_pixel(src, |x, y| {
        let (id, edge) = locate(x, y);
        let cell = cell_colour(sums[id]);
        if cell[3] == 0 {
            return cell;
        }
        pack(lerp3(rgb3(cell), seam, 0.85 * edge), cell[3])
    })
}

/// Mosaic tile border: a `color` frame cut into `tile` px squares (2 to 4096), each tinted a little
/// differently, with `grout` lines between them. The canvas grows by `width` on every side.
pub(super) fn mosaic_tile_border(src: &Buf, width: u32, tile: u32, color: Rgba, grout: Rgba, seed: u64) -> Result<Buf, EffectError> {
    let t = tile.clamp(2, MAX_CELL) as usize;
    let (base, grout) = (rgb_of(color), rgb_of(grout));
    frame(src, width, |x, y, _| {
        if x.is_multiple_of(t) || y.is_multiple_of(t) {
            return pack(grout, 255);
        }
        let id = (((y / t) as u64) << 32) | (x / t) as u64;
        let k = 0.7 + 0.5 * unit_hash(seed, id);
        pack(base.map(|c| c * k), 255)
    })
}

/// Nebula starfield: fractal clouds of `nebula` screened over the image at `strength`, with stars. `density`
/// (0..=1) is the share of pixels that may hold a star, up to 2%.
pub(super) fn nebula_starfield(src: &Buf, density: f32, nebula: Rgba, strength: f32, seed: u64) -> Buf {
    let k = unit(strength);
    let stars = unit(density) * 0.02;
    let glow = rgb_of(nebula).map(|c| c / 255.0);
    let w = src.w;
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let cloud = smoothstep(0.42, 0.7, fbm(seed, fx / 40.0, fy / 40.0, 4)) * k;
        let i = (y * w + x) as u64;
        let star = if unit_hash(seed ^ STAR_SALT, i) < stars { unit_hash(seed ^ STAR_SALT ^ 1, i) } else { 0.0 };
        pack(std::array::from_fn(|c| screen(p[c], glow[c] * cloud) + 255.0 * star), p[3])
    })
}

/// Neon edge glow: the image dims to a third and the edges light up in `color`, with a glow `radius` px
/// wide (capped at 128) that spreads from them. `strength` scales the light.
pub(super) fn neon_edge_glow(src: &Buf, color: Rgba, strength: f32, radius: u32) -> Buf {
    let (w, h) = (src.w, src.h);
    let k = cap(strength, 4.0).max(0.0);
    let mag = edges(src);
    let mut glow = mag.clone();
    blur::box3(&mut glow, w, h, 1, radius.min(MAX_HALO_RADIUS) as usize);
    let neon = rgb_of(color);
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let i = y * w + x;
        let lit = (smoothstep(0.1, 0.5, mag[i]) + glow[i]) * k;
        pack(std::array::from_fn(|c| f32::from(p[c]) * 0.35 + neon[c] * lit), p[3])
    })
}

/// Neon glow border: a `color` tube runs halfway through a frame `width` px deep, bright at its core and
/// glowing out to the edges. The canvas grows by `width` on every side.
pub(super) fn neon_glow_border(src: &Buf, width: u32, color: Rgba) -> Result<Buf, EffectError> {
    let m = width as f32;
    let (c, core) = (rgb_of(color), lerp3(rgb_of(color), [255.0; 3], 0.6));
    frame(src, width, |_, _, depth| {
        // Distance from the tube's centre line.
        let d = (depth as f32 + 0.5 - m / 2.0).abs();
        let tube = (-(d / 1.2).powi(2)).exp();
        let halo = 0.8 * (-(d / (0.4 * m)).powi(2)).exp();
        pack(std::array::from_fn(|i| c[i] * halo + core[i] * tube), 255)
    })
}

/// Night vision: monochrome green with a brightness gain (`gain` 0..=1), grain (`grain` 0..=1) and a dark
/// circular vignette.
pub(super) fn night_vision(src: &Buf, gain: f32, grain: f32, seed: u64) -> Buf {
    let gain = 1.0 + 2.0 * unit(gain);
    let grain = unit(grain) * 40.0;
    let (w, h) = (src.w, src.h);
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let l = (luma(&p).powf(0.7) * gain).min(1.0) * 255.0;
        let n = (unit_hash(seed, (y * w + x) as u64) - 0.5) * grain;
        let (dx, dy) = ((x as f32 + 0.5 - cx) / cx, (y as f32 + 0.5 - cy) / cy);
        let vig = 1.0 - 0.7 * smoothstep(0.5, 1.6, dx * dx + dy * dy);
        let v = (l + n) * vig;
        pack([v * 0.12, v, v * 0.2], p[3])
    })
}

/// Oil slick interference: thin-film fringes that swell and shift across the image, screened over it at
/// `strength`. `scale` is the size of the film's swirls in px.
pub(super) fn oil_slick(src: &Buf, strength: f32, scale: u32, seed: u64) -> Buf {
    let k = unit(strength);
    let s = scale.clamp(2, MAX_CELL) as f32;
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        // Film thickness varies smoothly across the image, and each channel's fringe runs at its own rate.
        let thickness = 0.7 * value_noise(seed, fx / s, fy / s) + 0.3 * value_noise(seed ^ OIL_SALT, fx / (0.3 * s), fy / (0.3 * s));
        let fringe: [f32; 3] = std::array::from_fn(|c| 0.5 + 0.5 * (TAU * thickness * (2.0 + 0.7 * c as f32)).cos());
        pack(std::array::from_fn(|c| screen(p[c], fringe[c] * k * 0.8)), p[3])
    })
}

/// Old camera flash burn: a hot spot of flash at the centre burns to white and falls off fast, and the
/// image takes a sepia cast by `sepia` (0..=1).
pub(super) fn old_camera_flash(src: &Buf, strength: f32, sepia: f32) -> Buf {
    let k = unit(strength);
    let sep = unit(sepia);
    let (cx, cy) = (src.w as f32 / 2.0, src.h as f32 / 2.0);
    let half = cx.hypot(cy).max(f32::MIN_POSITIVE);
    const TONE: [f32; 3] = [1.08, 0.94, 0.72];
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let d = (x as f32 + 0.5 - cx).hypot(y as f32 + 0.5 - cy) / half;
        let burn = k * (1.0 - d).clamp(0.0, 1.0).powi(3) * 255.0;
        let l = luma(&p) * 255.0;
        let base = rgb3(p);
        pack(std::array::from_fn(|c| base[c] + (l * TONE[c] - base[c]) * sep + burn), p[3])
    })
}

/// Ornate scroll border: a `color` band `width` px deep with a repeating scroll of two rings in `accent`,
/// each with a dot, on a rail along the middle. The canvas grows by `width` on every side.
pub(super) fn ornate_scroll_border(src: &Buf, width: u32, color: Rgba, accent: Rgba) -> Result<Buf, EffectError> {
    let m = width as usize;
    let (cw, ch) = (src.w + 2 * m, src.h + 2 * m);
    let tile = (2 * m).max(6) as f32;
    let mid = width as f32 / 2.0;
    let r = (0.3 * width as f32).max(1.0);
    let (base, ornament) = (rgb_of(color), rgb_of(accent));
    frame(src, width, |x, y, depth| {
        let along = if y.min(ch - 1 - y) <= x.min(cw - 1 - x) { x } else { y };
        let u = (along as f32 + 0.5).rem_euclid(tile);
        let v = depth as f32 + 0.5;
        let left = (u - tile * 0.25).hypot(v - mid);
        let right = (u - tile * 0.75).hypot(v - mid);
        let ring = (left - r).abs() < 0.6 || (right - r).abs() < 0.6;
        let dot = left < 1.0 || right < 1.0;
        let rail = (v - mid).abs() < 0.5;
        if ring || dot || rail { pack(ornament, 255) } else { pack(base, 255) }
    })
}

/// Palette map: luma picks a colour from a three-stop gradient of `shadow`, `midtone` and `highlight`.
pub(super) fn palette_map(src: &Buf, shadow: Rgba, midtone: Rgba, highlight: Rgba) -> Buf {
    let stops = [(0.0, rgb_of(shadow)), (0.5, rgb_of(midtone)), (1.0, rgb_of(highlight))];
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        pack(ramp(luma(&p), &stops), p[3])
    })
}

/// Paper stencil mask: the dark parts become `ink` cut out of `paper`, with a stencil edge that bends with
/// the paper's blotches. `threshold` sets how dark a pixel must be, and `grain` the paper's tooth.
pub(super) fn paper_stencil(src: &Buf, paper: Rgba, ink: Rgba, threshold: f32, grain: f32, seed: u64) -> Buf {
    let (paper, ink) = (rgb_of(paper), rgb_of(ink));
    let threshold = unit(threshold);
    let grain = unit(grain);
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        // Fibres are fine noise in the paper; blotches are coarser and bend the cut-out edges.
        let fibre = value_noise(seed ^ PAPER_SALT, fx / 1.5, fy / 1.5);
        let blotch = value_noise(seed, fx / 8.0, fy / 8.0);
        let level = 0.8 * luma(&p) + 0.2 * blotch;
        let cut = level < threshold + 0.1 * (fibre - 0.5);
        let c = if cut { ink } else { paper };
        let tooth = 1.0 - 0.25 * grain * fibre;
        pack(c.map(|v| v * tooth), p[3])
    })
}

/// Pencil sketch: colour dodge of the grey image by its inverse, blurred `radius` px, so edges stay dark and
/// flat areas go white. `strength` mixes from white paper to the sketch.
pub(super) fn pencil_sketch(src: &Buf, strength: f32, radius: u32) -> Buf {
    let (w, h) = (src.w, src.h);
    let k = unit(strength);
    let grey: Vec<f32> = src.px.as_chunks::<4>().0.iter().map(|p| luma(p) * 255.0).collect();
    let mut inverse: Vec<f32> = grey.iter().map(|g| 255.0 - g).collect();
    blur::box3(&mut inverse, w, h, 1, radius.min(MAX_HALO_RADIUS) as usize);
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let i = y * w + x;
        let dodge = (grey[i] * 256.0 / (256.0 - inverse[i].min(254.0))).min(255.0);
        let v = 255.0 + (dodge - 255.0) * k;
        pack([v; 3], p[3])
    })
}
