//! Filters: borders, glitch, film, drawing and light effects. Each kernel is written from the
//! effect's name and what it does to an image, and shares the helpers in the parent module. Pixels are
//! straight RGBA8 in and out. Every kernel is pure and deterministic for its seed.

use std::f32::consts::{FRAC_1_SQRT_2, FRAC_PI_2, PI, SQRT_2, TAU};

use laqta_domain::color::Rgba;
use rayon::prelude::*;

use super::{Buf, Rng, blur, cap, cells, hash, shifted, to_u8, tone, unit};
use crate::effects::EffectError;

/// Largest canvas an effect may allocate, in pixels. Matches the budget in `canvas.rs`.
const MAX_CANVAS_PIXELS: u64 = 1 << 28;
/// Caps on parameters that would otherwise ask for unbounded work. Larger values render as the cap.
const MAX_RAYS: u32 = 1024;
const MAX_FLARE_REACH: u32 = 128;
const MAX_BLOOM_RADIUS: u32 = 256;
const MAX_GLITCH_BLOCKS: u32 = 1024;
const MAX_SPLATS: u32 = 64;
const MAX_STRANDS: u32 = 4096;
const MAX_HAIR_LENGTH: u32 = 1024;
const MAX_ETCH_RADIUS: u32 = 64;
const MAX_CLAY_RADIUS: u32 = 64;
const MAX_STICKER: u32 = 64;
const MAX_CELL: u32 = 4096;
/// Diamond cells are raised in size until the grid is at most this many cells a side.
const MAX_DIAMOND_GRID: usize = 1024;

/// Tile sizes of the pattern borders, in pixels.
const BRICK_W: usize = 16;
const BRICK_H: usize = 8;
const CANDY_STRIPE: usize = 4;
const KNOT_TILE: usize = 12;
const LINK_TILE: usize = 12;
const CIRCUIT_GRID: usize = 8;

/// Per-effect salts, so two effects given one seed don't draw the same numbers.
const BRICK_SALT: u64 = 0xB81C_0001;
const CIRCUIT_SALT: u64 = 0xC1C0_0002;
const CHALK_SALT: u64 = 0xC4A1_0003;
const SMEAR_SALT: u64 = 0x5BEA_0004;
const SHADE_SALT: u64 = 0x5BAD_0005;
const PRISM_SALT: u64 = 0x9A15_0006;

/// Screen angles of the cyan, magenta and yellow dot grids, in degrees.
const SCREEN_DEGREES: [f32; 3] = [15.0, 75.0, 45.0];

/// Anaglyph 3D: the red channel from a view shifted `parallax` px right, and green and blue from one
/// shifted the same distance left, for red and cyan glasses. Alpha is kept.
pub(super) fn anaglyph(src: &Buf, parallax: u32) -> Buf {
    let p = i64::from(parallax);
    let w = src.w;
    per_pixel(src, |x, y| {
        let left = px_at(src, shifted(x, -p, w), y);
        let right = px_at(src, shifted(x, p, w), y);
        [left[0], right[1], right[2], px_at(src, x, y)[3]]
    })
}

/// Anamorphic lens flare: pixels brighter than `threshold` (0..=1 luma) smear into horizontal streaks
/// up to `length` px long. The streaks are tinted `color` and added at `strength`.
pub(super) fn anamorphic_flare(src: &Buf, threshold: f32, length: u32, strength: f32, color: Rgba) -> Buf {
    let w = src.w;
    let threshold = unit(threshold).min(0.999);
    let reach = length.min(MAX_FLARE_REACH) as usize;
    let gain = unit(strength);
    let tint = rgb_of(color).map(|c| c / 255.0);
    let bright: Vec<f32> = luma_map(src).into_iter().map(|l| ((l - threshold) / (1.0 - threshold)).clamp(0.0, 1.0)).collect();
    let norm = 1.0 / (reach as f32 + 1.0);
    per_pixel(src, |x, y| {
        let row = &bright[y * w..(y + 1) * w];
        let lo = x.saturating_sub(reach);
        let hi = (x + reach).min(w - 1);
        // Brightness along the row, weighted by a falloff that is 1 at the centre and 0 past the reach.
        let mut flare = 0.0;
        for (sx, &b) in row.iter().enumerate().take(hi + 1).skip(lo) {
            let t = 1.0 - sx.abs_diff(x) as f32 * norm;
            flare += b * t * t;
        }
        let energy = flare * norm;
        let p = px_at(src, x, y);
        let base = rgb3(p);
        pack(std::array::from_fn(|c| base[c] + 255.0 * gain * tint[c] * energy), p[3])
    })
}

/// Anime speed lines: `lines` thin rays radiate from the centre, out from `inner` (0..=1 of the
/// half-diagonal). Each ray's width is a seeded fraction of its sector. Rays fade in toward the edge and
/// mix the image toward `color` by `strength`.
pub(super) fn anime_speed_lines(src: &Buf, lines: u32, inner: f32, strength: f32, color: Rgba, seed: u64) -> Buf {
    if lines == 0 {
        return src.clone();
    }
    let rays = lines.min(MAX_RAYS) as usize;
    let (w, h) = (src.w, src.h);
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let half = (cx * cx + cy * cy).sqrt().max(f32::MIN_POSITIVE);
    let inner = unit(inner).min(0.95);
    let strength = unit(strength);
    let target = rgb_of(color);
    let widths: Vec<f32> = (0..rays).map(|k| 0.1 + 0.4 * unit_hash(seed, k as u64)).collect();
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
        let t = (dx * dx + dy * dy).sqrt() / half;
        if t <= inner {
            return p;
        }
        let pos = (dy.atan2(dx) / TAU + 0.5) * rays as f32;
        let k = (pos as usize).min(rays - 1);
        let within = pos - k as f32;
        if (within - 0.5).abs() > widths[k] * 0.5 {
            return p;
        }
        let fade = ((t - inner) / (1.0 - inner)).clamp(0.0, 1.0);
        pack(lerp3(rgb3(p), target, strength * fade), p[3])
    })
}

/// Art deco border: concentric bands of `color` and `accent`, `width` px deep. The odd bands carry
/// a row of diamonds in `color`. The canvas grows by `width` on every side.
pub(super) fn art_deco_border(src: &Buf, width: u32, color: Rgba, accent: Rgba) -> Result<Buf, EffectError> {
    let band = (width as usize / 4).max(1);
    let period = 2 * band;
    frame(src, width, |x, y, depth| {
        let step = depth / band;
        let diamond = (x % period).abs_diff(band) + (y % period).abs_diff(band) < band;
        if step.is_multiple_of(2) || diamond { rgba4(color) } else { rgba4(accent) }
    })
}

/// ASCII art: each `cell` px square becomes a glyph chosen by its brightness, in `color` on black. The
/// glyph is one of eight procedural shapes from blank to solid. Alpha follows the cell.
pub(super) fn ascii_art(src: &Buf, cell: u32, color: Rgba) -> Buf {
    let cell = cell.clamp(1, MAX_CELL);
    let avg = cells::pixelate(src, cell);
    let s = cell as usize;
    let ink = rgba4(color);
    per_pixel(src, |x, y| {
        let q = px_at(&avg, x, y);
        let level = (luma(&q) * f32::from(q[3]) / 255.0 * 8.0).floor().min(7.0) as u8;
        let u = ((x % s) as f32 + 0.5) / s as f32 - 0.5;
        let v = ((y % s) as f32 + 0.5) / s as f32 - 0.5;
        let c = if glyph(level, u, v) { ink } else { [0, 0, 0, 255] };
        [c[0], c[1], c[2], q[3]]
    })
}

/// Whether the glyph for `level` (0 blank, 7 solid) covers the point (`u`, `v`), each in -0.5..=0.5.
fn glyph(level: u8, u: f32, v: f32) -> bool {
    let (a, b) = (u.abs(), v.abs());
    match level {
        0 => false,
        1 => a < 0.12 && b < 0.12,
        2 => b < 0.1 && a < 0.36,
        3 => (b < 0.1 && a < 0.36) || (a < 0.1 && b < 0.36),
        4 => (u - v).abs() < 0.14 || (u + v).abs() < 0.14,
        5 => a.max(b) > 0.2 && a.max(b) < 0.38,
        6 => a.max(b) < 0.38,
        _ => true,
    }
}

/// Bevel: the outer `size` px are lit on the top and left and shaded on the bottom and right, by
/// `strength` at the very edge, fading to nothing at `size` px in.
pub(super) fn bevel(src: &Buf, size: u32, strength: f32) -> Buf {
    let size = size as usize;
    let strength = unit(strength);
    let (w, h) = (src.w, src.h);
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let (tl, br) = (x.min(y), (w - 1 - x).min(h - 1 - y));
        let depth = tl.min(br);
        if depth >= size || tl == br {
            return p;
        }
        let edge = if tl < br { 255.0 } else { 0.0 };
        let k = strength * (1.0 - depth as f32 / size as f32);
        pack(lerp3(rgb3(p), [edge; 3], k), p[3])
    })
}

/// Block glitch: `count` random blocks are copied `max_shift` px or less to the side, leaving the
/// original in place. Blocks come from one seeded stream, so the output depends only on the seed.
pub(super) fn block_glitch(src: &Buf, count: u32, max_shift: u32, seed: u64) -> Buf {
    let mut out = src.clone();
    if count == 0 || max_shift == 0 {
        return out;
    }
    let (w, h) = (src.w, src.h);
    let (w32, h32) = (w as u32, h as u32);
    let mut rng = Rng::new(seed);
    for _ in 0..count.min(MAX_GLITCH_BLOCKS) {
        let bw = rng.range(1, (w32 / 2).max(1));
        let bh = rng.range(1, (h32 / 6).max(1));
        let x0 = rng.range(0, w32 - bw) as usize;
        let y0 = rng.range(0, h32 - bh) as usize;
        let (bw, bh) = (bw as usize, bh as usize);
        let sign = if rng.next_u64() & 1 == 0 { 1 } else { -1 };
        let d = i64::from(rng.range(1, max_shift)) * sign;
        for y in y0..y0 + bh {
            for x in x0..x0 + bw {
                let tx = x as i64 + d;
                if (0..w as i64).contains(&tx) {
                    let dst = (y * w + tx as usize) * 4;
                    let from = (y * w + x) * 4;
                    out.px[dst..dst + 4].copy_from_slice(&src.px[from..from + 4]);
                }
            }
        }
    }
    out
}

/// One blood splat: a wobbly blob at (`x`, `y`) of radius `r`, with a drip running down from it.
struct Splat {
    x: f32,
    y: f32,
    r: f32,
    phase: f32,
    drip_len: f32,
    drip_w: f32,
}

/// Blood splash: `count` splats of `size` (0..=1 of the shorter side) are painted in `color`, each
/// brighter near its middle, with a drip below. Splats are placed from the seed.
pub(super) fn blood_splash(src: &Buf, count: u32, size: f32, color: Rgba, seed: u64) -> Buf {
    let size = unit(size);
    if count == 0 || size <= 0.0 {
        return src.clone();
    }
    let (w, h) = (src.w as f32, src.h as f32);
    let base = w.min(h) * size;
    let splats: Vec<Splat> = (0..u64::from(count.min(MAX_SPLATS)))
        .map(|k| {
            let key = |slot: u64| unit_hash(seed, 8 * k + slot);
            let r = base * (0.5 + 0.5 * key(2));
            Splat { x: key(0) * w, y: key(1) * h, r, phase: key(3) * TAU, drip_len: r * (1.0 + 3.0 * key(4)), drip_w: (r * 0.12).max(0.5) }
        })
        .collect();
    let target = rgb_of(color);
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let mut best = 0.0f32;
        let mut paint = [0.0f32; 3];
        for s in &splats {
            let (dx, dy) = (fx - s.x, fy - s.y);
            let dist = dx.hypot(dy);
            let theta = dy.atan2(dx);
            // The rim wobbles with the angle, so a splat is a blob rather than a disc.
            let rim = s.r * (0.82 + 0.12 * (3.0 * theta + s.phase).sin() + 0.06 * (7.0 * theta - 2.0 * s.phase).sin());
            let blob = smoothstep(rim + 0.5, rim - 0.5, dist);
            let drip = dx.abs() < s.drip_w && fy > s.y && fy < s.y + s.drip_len;
            let mask = if drip { 1.0 } else { blob };
            if mask > best {
                best = mask;
                let k = 0.7 + 0.5 * smoothstep(s.r, 0.0, dist);
                paint = target.map(|c| c * k);
            }
        }
        if best <= 0.0 {
            return p;
        }
        pack(lerp3(rgb3(p), paint, best), p[3])
    })
}

/// Bloom: the parts brighter than `threshold` (0..=1 luma) are blurred over `radius` px and added back
/// at `strength` (up to 8).
pub(super) fn bloom(src: &Buf, threshold: f32, radius: u32, strength: f32) -> Buf {
    let (w, h) = (src.w, src.h);
    let threshold = unit(threshold).min(0.999);
    let gain = cap(strength, 8.0).max(0.0);
    let mut bright = vec![0f32; w * h * 3];
    for (i, p) in src.px.as_chunks::<4>().0.iter().enumerate() {
        let k = ((luma(p) - threshold) / (1.0 - threshold)).clamp(0.0, 1.0) * f32::from(p[3]) / 255.0;
        for (b, v) in bright[i * 3..i * 3 + 3].iter_mut().zip(rgb3(*p)) {
            *b = v * k;
        }
    }
    blur::box3(&mut bright, w, h, 3, radius.min(MAX_BLOOM_RADIUS) as usize);
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let base = rgb3(p);
        let i = (y * w + x) * 3;
        pack(std::array::from_fn(|c| base[c] + gain * bright[i + c]), p[3])
    })
}

/// Blueprint drawing: edges in `line` over a `background` that has a faint grid every 8 px.
pub(super) fn blueprint_drawing(src: &Buf, background: Rgba, line: Rgba) -> Buf {
    let mag = edges(src);
    let (bg, ln) = (rgb_of(background), rgb_of(line));
    let w = src.w;
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let ground = if x.is_multiple_of(8) || y.is_multiple_of(8) { lerp3(bg, [255.0; 3], 0.08) } else { bg };
        let e = smoothstep(0.1, 0.5, mag[y * w + x]);
        pack(lerp3(ground, ln, e), p[3])
    })
}

/// Brick wall border: a running bond of `BRICK_W` x `BRICK_H` bricks in `brick`, with `mortar` lines.
/// Each brick varies a little from its neighbours.
pub(super) fn brick_wall_border(src: &Buf, width: u32, brick: Rgba, mortar: Rgba) -> Result<Buf, EffectError> {
    frame(src, width, |x, y, _| {
        let row = y / BRICK_H;
        let offset = if row.is_multiple_of(2) { 0 } else { BRICK_W / 2 };
        let col = (x + offset) / BRICK_W;
        if y.is_multiple_of(BRICK_H) || (x + offset).is_multiple_of(BRICK_W) {
            return rgba4(mortar);
        }
        let k = 1.0 + 0.16 * (unit_hash(BRICK_SALT, (row * 4096 + col) as u64) - 0.5);
        shade(brick, k)
    })
}

/// Candy cane border: diagonal stripes of `color` and `accent`, `CANDY_STRIPE` px wide.
pub(super) fn candy_cane_border(src: &Buf, width: u32, color: Rgba, accent: Rgba) -> Result<Buf, EffectError> {
    frame(src, width, |x, y, _| if ((x + y) / CANDY_STRIPE).is_multiple_of(2) { rgba4(color) } else { rgba4(accent) })
}

/// Cartoon sticker cutout: the image posterised to `levels` with dark lines on its strong edges, on a
/// white (`color`) sticker outline `outline` px wide around its opaque shape. The canvas grows by the outline.
pub(super) fn cartoon_sticker(src: &Buf, levels: u8, outline: u32, color: Rgba) -> Result<Buf, EffectError> {
    let margin = outline.min(MAX_STICKER) as usize;
    let (w, h) = (src.w, src.h);
    let (cw, ch) = grown(w, h, margin)?;
    let posterised = tone::posterize(src, levels);
    let mag = edges(src);
    let art = per_pixel(src, |x, y| {
        let p = px_at(&posterised, x, y);
        let dark = smoothstep(0.35, 0.6, mag[y * w + x]);
        pack(lerp3(rgb3(p), [0.0; 3], dark), p[3])
    });
    // A summed-area table of the opaque pixels, so each canvas pixel can ask for an opaque neighbour in
    // constant time.
    let stride = w + 1;
    let mut sat = vec![0u32; stride * (h + 1)];
    for y in 0..h {
        for x in 0..w {
            let opaque = u32::from(src.px[(y * w + x) * 4 + 3] >= 128);
            let (up, left, diag) = (sat[y * stride + x + 1], sat[(y + 1) * stride + x], sat[y * stride + x]);
            sat[(y + 1) * stride + x + 1] = opaque + up + left - diag;
        }
    }
    let window = |x0: i64, y0: i64, x1: i64, y1: i64| -> u32 {
        let (x0, y0) = (x0.max(0), y0.max(0));
        let (x1, y1) = (x1.min(w as i64 - 1), y1.min(h as i64 - 1));
        if x0 > x1 || y0 > y1 {
            return 0;
        }
        let (x0, y0, x1, y1) = (x0 as usize, y0 as usize, x1 as usize, y1 as usize);
        let at = |x: usize, y: usize| sat[y * stride + x];
        (at(x1 + 1, y1 + 1) + at(x0, y0)) - (at(x1 + 1, y0) + at(x0, y1 + 1))
    };
    let m = margin as i64;
    let mut px = vec![0u8; cw * ch * 4];
    px.par_chunks_mut(cw * 4).enumerate().for_each(|(cy, row)| {
        for (cx, out) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let (ix, iy) = (cx as i64 - m, cy as i64 - m);
            let inside = ix >= 0 && iy >= 0 && (ix as usize) < w && (iy as usize) < h;
            let own = if inside { px_at(&art, ix as usize, iy as usize) } else { [0, 0, 0, 0] };
            let shape = inside && src.px[((iy as usize) * w + ix as usize) * 4 + 3] >= 128;
            let near = window(ix - m, iy - m, ix + m, iy + m) > 0;
            *out = if shape {
                own
            } else if near {
                rgba4(color)
            } else {
                own
            };
        }
    });
    Ok(Buf { w: cw, h: ch, px })
}

/// Celtic knot border: arcs of radius half a tile, one about each tile corner, alternately `color` and
/// `accent` so they weave over and under each other. The gaps are a dark `color`.
pub(super) fn celtic_knot_border(src: &Buf, width: u32, color: Rgba, accent: Rgba) -> Result<Buf, EffectError> {
    let t = KNOT_TILE as f32;
    frame(src, width, |x, y, _| {
        let (u, v) = ((x % KNOT_TILE) as f32 + 0.5, (y % KNOT_TILE) as f32 + 0.5);
        let tile = (x / KNOT_TILE + y / KNOT_TILE) % 2;
        let corners: [(f32, f32); 4] = [(0.0, 0.0), (t, 0.0), (0.0, t), (t, t)];
        for (k, (cx, cy)) in corners.into_iter().enumerate() {
            let d = (u - cx).hypot(v - cy);
            if (d - t / 2.0).abs() < 1.2 {
                return if (k + tile).is_multiple_of(2) { rgba4(color) } else { rgba4(accent) };
            }
        }
        shade(color, 0.3)
    })
}

/// Chain link border: a grid of oval links, `color` on an `accent` ground. Links alternate upright and
/// flat from tile to tile, so each one passes through its neighbours.
pub(super) fn chain_link_border(src: &Buf, width: u32, color: Rgba, accent: Rgba) -> Result<Buf, EffectError> {
    let s = LINK_TILE as f32;
    frame(src, width, |x, y, _| {
        let (u, v) = ((x % LINK_TILE) as f32 + 0.5 - s / 2.0, (y % LINK_TILE) as f32 + 0.5 - s / 2.0);
        let upright = (x / LINK_TILE + y / LINK_TILE).is_multiple_of(2);
        let (a, b) = if upright { (s * 0.42, s * 0.22) } else { (s * 0.22, s * 0.42) };
        let e = (u / a).powi(2) + (v / b).powi(2);
        if (0.45..=1.0).contains(&e) { rgba4(color) } else { rgba4(accent) }
    })
}

/// Chalk drawing: the edges as chalk strokes on a `board` with a faint mottling. Chalk lies on where an
/// edge is stronger than `threshold` (0..=1), and grains out in places.
pub(super) fn chalk_drawing(src: &Buf, threshold: f32, board: Rgba, chalk: Rgba, seed: u64) -> Buf {
    let mag = edges(src);
    let threshold = unit(threshold);
    let (board, chalk) = (rgb_of(board), rgb_of(chalk));
    let w = src.w;
    per_pixel(src, |x, y| {
        let i = y * w + x;
        let p = px_at(src, x, y);
        let mottle = 0.94 + 0.12 * unit_hash(seed, i as u64);
        let dust = if unit_hash(seed ^ CHALK_SALT, i as u64) < 0.25 { 0.45 } else { 1.0 };
        let cover = smoothstep(threshold, threshold + 0.25, mag[i]) * dust;
        pack(lerp3(board.map(|c| c * mottle), chalk, cover), p[3])
    })
}

/// Chromatic aberration: red is sampled further out from the centre and blue nearer it, by a share of
/// the offset that grows toward the edges (`amount` 0..=1 sets the share at the corners). Green stays.
pub(super) fn chromatic_aberration(src: &Buf, amount: f32) -> Buf {
    let k = unit(amount) * 0.25;
    let (w, h) = (src.w, src.h);
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let half = (cx * cx + cy * cy).sqrt().max(f32::MIN_POSITIVE);
    per_pixel(src, |x, y| {
        let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
        let s = k * ((dx * dx + dy * dy).sqrt() / half).min(1.0);
        let red = sample(src, [x as f32 + 0.5 + dx * s, y as f32 + 0.5 + dy * s]);
        let blue = sample(src, [x as f32 + 0.5 - dx * s, y as f32 + 0.5 - dy * s]);
        let p = px_at(src, x, y);
        [red[0], p[1], blue[2], p[3]]
    })
}

/// Chrome metallic border: a brushed `color` frame with two bright bands across its depth and a faint
/// streak along it.
pub(super) fn chrome_border(src: &Buf, width: u32, color: Rgba) -> Result<Buf, EffectError> {
    let w = width.max(1) as f32;
    frame(src, width, |x, y, depth| {
        let t = (depth as f32 + 0.5) / w;
        let sheen = 0.5 + 0.5 * (2.0 * TAU * t).cos();
        let streak = if (x + y).is_multiple_of(3) { 0.97 } else { 1.0 };
        shade(color, (0.4 + 0.75 * sheen) * streak)
    })
}

/// Circuit board border: a `board` ground with `trace` lines on some grid lines and a 3x3 pad on some
/// cells.
pub(super) fn circuit_border(src: &Buf, width: u32, board: Rgba, trace: Rgba) -> Result<Buf, EffectError> {
    let mid = CIRCUIT_GRID / 2;
    frame(src, width, |x, y, _| {
        let (gx, gy) = (x / CIRCUIT_GRID, y / CIRCUIT_GRID);
        let (cx, cy) = (x % CIRCUIT_GRID, y % CIRCUIT_GRID);
        let h_trace = cy == mid && unit_hash(CIRCUIT_SALT, gy as u64) < 0.6;
        let v_trace = cx == mid && unit_hash(CIRCUIT_SALT ^ 1, gx as u64) < 0.6;
        let pad = cx.abs_diff(mid) <= 1 && cy.abs_diff(mid) <= 1 && unit_hash(CIRCUIT_SALT ^ 2, (gy * 4096 + gx) as u64) < 0.5;
        if h_trace || v_trace || pad { rgba4(trace) } else { rgba4(board) }
    })
}

/// Claymation texture: a soft, blurred colour lit from the top left as if it were a modelled surface.
/// `radius` px sets the softness and `depth` how strong the relief is.
pub(super) fn claymation(src: &Buf, radius: u32, depth: f32) -> Buf {
    let soft = blur::box_blur(src, radius.min(MAX_CLAY_RADIUS));
    let (w, h) = (src.w, src.h);
    let depth = cap(depth, 16.0).max(0.0);
    let lum = luma_map(&soft);
    let at = |x: usize, y: usize| lum[y * w + x];
    let light = [-0.5f32, -0.5, FRAC_1_SQRT_2];
    per_pixel(src, |x, y| {
        let (xl, xr) = (x.saturating_sub(1), (x + 1).min(w - 1));
        let (yu, yd) = (y.saturating_sub(1), (y + 1).min(h - 1));
        let gx = (at(xr, y) - at(xl, y)) * 0.5 * depth;
        let gy = (at(x, yd) - at(x, yu)) * 0.5 * depth;
        let len = (gx * gx + gy * gy + 1.0).sqrt();
        let normal = [-gx / len, -gy / len, 1.0 / len];
        let diffuse = (normal[0] * light[0] + normal[1] * light[1] + normal[2] * light[2]).max(0.0);
        let k = 0.6 + 0.6 * diffuse;
        pack(rgb3(px_at(&soft, x, y)).map(|v| v * k), px_at(src, x, y)[3])
    })
}

/// Clouds: fractal value noise on a lattice `scale` px apart, with four octaves. Where the noise is
/// dense it mixes the image toward `color` by `opacity`.
pub(super) fn clouds(src: &Buf, scale: u32, opacity: f32, color: Rgba, seed: u64) -> Buf {
    let scale = scale.clamp(2, MAX_CELL) as f32;
    let opacity = unit(opacity);
    let target = rgb_of(color);
    per_pixel(src, |x, y| {
        let (fx, fy) = ((x as f32 + 0.5) / scale, (y as f32 + 0.5) / scale);
        let (mut sum, mut weight, mut amp) = (0.0, 0.0, 1.0);
        for o in 0..4u64 {
            let f = (1u64 << o) as f32;
            sum += amp * value_noise(hash(seed, o), fx * f, fy * f);
            weight += amp;
            amp *= 0.5;
        }
        let t = smoothstep(0.45, 0.7, sum / weight) * opacity;
        let p = px_at(src, x, y);
        pack(lerp3(rgb3(p), target, t), p[3])
    })
}

/// CMYK colour halftone: three rotated grids of `cell` px dots, one each for cyan, magenta and yellow.
/// A dot's area follows how much of its ink the pixel has.
pub(super) fn color_halftone(src: &Buf, cell: u32) -> Buf {
    let cell = cell.clamp(1, MAX_CELL) as f32;
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let inked = |c: usize| {
            // Each channel's ink is the darkness it lacks: red shows cyan, green magenta, blue yellow.
            let ink = 1.0 - f32::from(p[c]) / 255.0;
            let (sin, cos) = SCREEN_DEGREES[c].to_radians().sin_cos();
            let u = fx * cos + fy * sin;
            let v = fy * cos - fx * sin;
            let (cu, cv) = (((u / cell).floor() + 0.5) * cell, ((v / cell).floor() + 0.5) * cell);
            let radius = cell * (ink / PI).sqrt();
            (u - cu).powi(2) + (v - cv).powi(2) <= radius * radius
        };
        pack(std::array::from_fn(|c| if inked(c) { 0.0 } else { 255.0 }), p[3])
    })
}

/// Colour splash: colours within `tolerance` (0..=1 of the RGB distance) of `target` keep their colour,
/// and the rest go grey, with a short blend between the two.
pub(super) fn color_splash(src: &Buf, target: Rgba, tolerance: f32) -> Buf {
    let tol = unit(tolerance);
    let t = rgb_of(target);
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let base = rgb3(p);
        let d = (0..3).map(|c| (base[c] - t[c]).powi(2)).sum::<f32>().sqrt() / (255.0 * 3f32.sqrt());
        let keep = 1.0 - smoothstep(tol, tol + 0.1, d);
        let grey = luma(&p) * 255.0;
        pack(std::array::from_fn(|c| grey + (base[c] - grey) * keep), p[3])
    })
}

/// Crimson border: solid `color` rules at the outer and inner edges, a dotted `accent` line down the
/// middle of the band, and a dark `color` field between.
pub(super) fn crimson_border(src: &Buf, width: u32, color: Rgba, accent: Rgba) -> Result<Buf, EffectError> {
    let w = width as usize;
    frame(src, width, |x, y, depth| {
        if depth < 2 || depth + 2 >= w {
            rgba4(color)
        } else if depth == w / 2 && (x + y).is_multiple_of(4) {
            rgba4(accent)
        } else {
            shade(color, 0.55)
        }
    })
}

/// Crosshatch: a pen drawing on white. Darker pixels get more hatch layers: one diagonal above 0.2 darkness, the
/// other diagonal above 0.45, and horizontal lines above 0.7. Lines are `spacing` px apart.
pub(super) fn crosshatch(src: &Buf, spacing: u32) -> Buf {
    let s = spacing.clamp(2, 64) as usize;
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let dark = 1.0 - luma(&p);
        let hatched = (dark > 0.2 && (x + y).is_multiple_of(s))
            || (dark > 0.45 && (x + s - y % s).is_multiple_of(s))
            || (dark > 0.7 && y.is_multiple_of(s));
        let v = if hatched { 0 } else { 255 };
        [v, v, v, p[3]]
    })
}

/// CRT: scanlines darken every other row by `scanlines`, a red-green-blue phosphor mask dims the
/// sub-pixels that aren't lit in each column by `mask`, and the corners darken by `vignette`. All 0..=1.
pub(super) fn crt(src: &Buf, scanlines: f32, mask: f32, vignette: f32) -> Buf {
    let (scan, mask, vignette) = (unit(scanlines), unit(mask), unit(vignette));
    let (w, h) = (src.w, src.h);
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let line = if y.is_multiple_of(2) { 1.0 } else { 1.0 - scan };
        let lit = x % 3;
        let (dx, dy) = ((x as f32 + 0.5 - cx) / cx, (y as f32 + 0.5 - cy) / cy);
        let corner = smoothstep(0.6, 2.0, dx * dx + dy * dy);
        let vig = 1.0 - vignette * 0.9 * corner;
        let base = rgb3(p);
        pack(std::array::from_fn(|c| base[c] * line * vig * (if c == lit { 1.0 } else { 1.0 - mask * 0.6 })), p[3])
    })
}

/// Crystalize shards: the image cut into triangular facets of `size` px cells, each taking the colour
/// under its centre and lit a little differently from its neighbours.
pub(super) fn shards(src: &Buf, size: u32, seed: u64) -> Buf {
    let mesh = Mesh { size: size.clamp(2, MAX_CELL) as f32, seed };
    per_pixel(src, |x, y| {
        let (i, j, k) = mesh.locate([x as f32 + 0.5, y as f32 + 0.5]);
        let centre = centroid(mesh.triangle(i, j, k));
        let light = 0.85 + 0.3 * unit_hash(seed ^ SHADE_SALT, facet_key(i, j, k));
        pack(rgb3(sample(src, centre)).map(|v| v * light), px_at(src, x, y)[3])
    })
}

/// Crystal prism: the facets of `shards`, with the red and blue samples slid apart along each facet's
/// own direction by `dispersion` (0..=1 of a cell), so the edges fringe like light through glass.
pub(super) fn prism(src: &Buf, size: u32, dispersion: f32, seed: u64) -> Buf {
    let mesh = Mesh { size: size.clamp(2, MAX_CELL) as f32, seed };
    let spread = unit(dispersion) * mesh.size * 0.5;
    per_pixel(src, |x, y| {
        let (i, j, k) = mesh.locate([x as f32 + 0.5, y as f32 + 0.5]);
        let c = centroid(mesh.triangle(i, j, k));
        let (sin, cos) = (unit_hash(seed ^ PRISM_SALT, facet_key(i, j, k)) * TAU).sin_cos();
        let (dx, dy) = (cos * spread, sin * spread);
        let red = sample(src, [c[0] + dx, c[1] + dy]);
        let green = sample(src, c);
        let blue = sample(src, [c[0] - dx, c[1] - dy]);
        let light = 0.85 + 0.3 * unit_hash(seed ^ SHADE_SALT, facet_key(i, j, k));
        let rgb = [f32::from(red[0]) * light, f32::from(green[1]) * light, f32::from(blue[2]) * light];
        pack(rgb, px_at(src, x, y)[3])
    })
}

/// Cyanotype: a two-colour print from `shadow` to `highlight`, through a smooth S-curve on the luma.
pub(super) fn cyanotype(src: &Buf, shadow: Rgba, highlight: Rgba) -> Buf {
    let (shadow, highlight) = (rgb_of(shadow), rgb_of(highlight));
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let t = smoothstep(0.0, 1.0, luma(&p));
        pack(lerp3(shadow, highlight, t), p[3])
    })
}

/// Datamosh smear: the image is cut into `block` px cells, and about half of them drag their pixels
/// sideways by up to `max_shift` px, as a broken video codec smears motion.
pub(super) fn datamosh(src: &Buf, block: u32, max_shift: u32, seed: u64) -> Buf {
    let block = block.clamp(1, MAX_CELL) as usize;
    let max = max_shift.min(MAX_CELL) as f32;
    let w = src.w;
    per_pixel(src, |x, y| {
        let (bx, by) = (x / block, y / block);
        let id = ((by as u64) << 32) | bx as u64;
        if max == 0.0 || unit_hash(seed, id) < 0.5 {
            return px_at(src, x, y);
        }
        let shift = ((unit_hash(seed ^ SMEAR_SALT, id) * 2.0 - 1.0) * max).round() as i64;
        px_at(src, shifted(x, shift, w), y)
    })
}

/// Diamond pixelate: the average colour over diamond-shaped cells about `size` px across. Sizes are
/// raised for very large images so the grid stays at most `MAX_DIAMOND_GRID` cells a side.
pub(super) fn diamond_pixelate(src: &Buf, size: u32) -> Buf {
    let (w, h) = (src.w, src.h);
    let s = (size.clamp(1, 1 << 16) as usize).max((w + h) / MAX_DIAMOND_GRID + 1);
    let n = (w + h - 2) / s + 1;
    // Diagonal coordinates: a = x + y and b = x - y + h - 1 are both in 0..=w + h - 2, and the cells are
    // s by s in (a, b). A cell's pixels form a diamond in the image.
    let cell_of = |x: usize, y: usize| ((x + y) / s) * n + (x + h - 1 - y) / s;
    // Per cell: alpha-weighted sums of red, green and blue, the alpha sum, and the pixel count.
    let mut acc = vec![[0f32; 5]; n * n];
    for y in 0..h {
        for x in 0..w {
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
    per_pixel(src, |x, y| {
        let c = acc[cell_of(x, y)];
        if c[3] <= 0.0 {
            return [0, 0, 0, 0];
        }
        pack([c[0] / c[3], c[1] / c[3], c[2] / c[3]], to_u8(c[3] / c[4]))
    })
}

/// Disposable camera: a warm cast (`warmth`), monochrome grain strongest in the midtones (`grain`),
/// and a darkened corner vignette (`vignette`). All 0..=1.
pub(super) fn disposable_camera(src: &Buf, warmth: f32, grain: f32, vignette: f32, seed: u64) -> Buf {
    let (warmth, grain, vignette) = (unit(warmth), unit(grain), unit(vignette));
    let (w, h) = (src.w, src.h);
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let l = luma(&p);
        let n = (unit_hash(seed, (y * w + x) as u64) - 0.5) * 2.0 * grain * 40.0 * (4.0 * l * (1.0 - l)).max(0.3);
        let (dx, dy) = ((x as f32 + 0.5 - cx) / cx, (y as f32 + 0.5 - cy) / cy);
        let vig = 1.0 - vignette * 0.7 * smoothstep(0.3, 2.0, dx * dx + dy * dy);
        let warm = [1.0 + 0.12 * warmth, 1.0 + 0.04 * warmth, 1.0 - 0.15 * warmth];
        let base = rgb3(p);
        pack(std::array::from_fn(|c| (base[c] * warm[c] + n) * vig), p[3])
    })
}

/// Dot matrix: an LED panel. Each `cell` px square has a round dot in its middle in the cell's average
/// colour, and the gaps show `background`.
pub(super) fn dot_matrix(src: &Buf, cell: u32, background: Rgba) -> Buf {
    let cell = cell.clamp(1, MAX_CELL);
    let avg = cells::pixelate(src, cell);
    let s = cell as usize;
    let radius = s as f32 * 0.4;
    let bg = rgba4(background);
    per_pixel(src, |x, y| {
        let (dx, dy) = ((x % s) as f32 + 0.5 - s as f32 / 2.0, (y % s) as f32 + 0.5 - s as f32 / 2.0);
        if dx * dx + dy * dy > radius * radius {
            return bg;
        }
        px_at(&avg, x, y)
    })
}

/// Double exposure: a second copy, mirrored left to right and moved by the offset, is screened over
/// the image at `opacity`.
pub(super) fn double_exposure(src: &Buf, offset: (i32, i32), opacity: f32) -> Buf {
    let opacity = unit(opacity);
    let (w, h) = (src.w, src.h);
    let (ox, oy) = (i64::from(offset.0), i64::from(offset.1));
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let q = px_at(src, shifted(w - 1 - x, -ox, w), shifted(y, -oy, h));
        let s = opacity * f32::from(q[3]) / 255.0;
        pack(std::array::from_fn(|c| screen(p[c], f32::from(q[c]) / 255.0 * s)), p[3])
    })
}

/// Dream glow: a box-blurred copy of the image, `radius` px, screened over the image at `strength`
/// (up to 4).
pub(super) fn dream_glow(src: &Buf, radius: u32, strength: f32) -> Buf {
    let strength = cap(strength, 4.0).max(0.0);
    let soft = blur::box_blur(src, radius.min(MAX_BLOOM_RADIUS));
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let q = px_at(&soft, x, y);
        let s = strength * f32::from(q[3]) / 255.0;
        pack(std::array::from_fn(|c| screen(p[c], f32::from(q[c]) / 255.0 * s)), p[3])
    })
}

/// Edge feather: alpha fades to nothing over the outer `size` px, so the edges dissolve.
pub(super) fn edge_feather(src: &Buf, size: u32) -> Buf {
    let (w, h) = (src.w, src.h);
    let size = size as f32;
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let d = x.min(y).min(w - 1 - x).min(h - 1 - y) as f32 + 0.5;
        let keep = smoothstep(0.0, size, d);
        [p[0], p[1], p[2], to_u8(f32::from(p[3]) * keep)]
    })
}

/// Embroidery: the image as cross stitches of `cell` px, each stitch the cell's average colour on a
/// `cloth` ground. One diagonal of each stitch is a little darker.
pub(super) fn embroidery(src: &Buf, cell: u32, cloth: Rgba) -> Buf {
    let cell = cell.clamp(2, MAX_CELL);
    let avg = cells::pixelate(src, cell);
    let s = cell as usize;
    let sf = cell as f32;
    let thick = sf * 0.2;
    let cloth = rgba4(cloth);
    per_pixel(src, |x, y| {
        let (u, v) = ((x % s) as f32 + 0.5, (y % s) as f32 + 0.5);
        let back = (u - v).abs() / SQRT_2 < thick;
        let front = (u + v - sf).abs() / SQRT_2 < thick;
        let q = px_at(&avg, x, y);
        if (!back && !front) || q[3] == 0 {
            return cloth;
        }
        let k = if back { 1.0 } else { 0.8 };
        pack(rgb3(q).map(|c| c * k), 255)
    })
}

/// Engraving: horizontal lines `spacing` px apart on `paper`, each drawn in `ink`. The lines swell with
/// darkness until the darkest areas are solid.
pub(super) fn engraving(src: &Buf, spacing: u32, paper: Rgba, ink: Rgba) -> Buf {
    let s = spacing.clamp(2, 64) as usize;
    let (paper, ink) = (rgba4(paper), rgba4(ink));
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let dark = 1.0 - luma(&p);
        let offset = (y % s) as f32 + 0.5 - s as f32 / 2.0;
        let on = offset.abs() < dark * s as f32 / 2.0;
        let c = if on { ink } else { paper };
        [c[0], c[1], c[2], p[3]]
    })
}

/// Etched glass: each pixel takes the colour of a neighbour within `radius` px, picked from the seed,
/// and is lifted slightly toward white.
pub(super) fn etched_glass(src: &Buf, radius: u32, seed: u64) -> Buf {
    let r = radius.min(MAX_ETCH_RADIUS) as usize;
    let (w, h) = (src.w, src.h);
    let span = (2 * r + 1) as f32;
    per_pixel(src, |x, y| {
        let i = (y * w + x) as u64;
        let dx = (unit_hash(seed, 2 * i) * span) as i64 - r as i64;
        let dy = (unit_hash(seed, 2 * i + 1) * span) as i64 - r as i64;
        let q = px_at(src, shifted(x, -dx, w), shifted(y, -dy, h));
        let p = px_at(src, x, y);
        pack(lerp3(rgb3(q), [255.0; 3], 0.12), p[3])
    })
}

/// Fake hair: `strands` thin strands of `length` px in `color`, each starting near the top and curling
/// downward from its own seeded heading. They are drawn over the image and stop at its edge.
pub(super) fn fake_hair(src: &Buf, strands: u32, length: u32, color: Rgba, seed: u64) -> Buf {
    let mut out = src.clone();
    let (w, h) = (src.w as f32, src.h as f32);
    let len = length.min(MAX_HAIR_LENGTH);
    let col = rgb_of(color);
    for k in 0..u64::from(strands.min(MAX_STRANDS)) {
        let key = |slot: u64| unit_hash(seed, 4 * k + slot);
        let (mut x, mut y) = (key(0) * w, key(1) * h * 0.4);
        let mut heading = (key(2) - 0.5) * FRAC_PI_2;
        let curl = (key(3) - 0.5) * 0.25;
        for _ in 0..len {
            let (xi, yi) = (x.floor() as i64, y.floor() as i64);
            if xi < 0 || yi < 0 || xi >= src.w as i64 || yi >= src.h as i64 {
                break;
            }
            let (xu, yu) = (xi as usize, yi as usize);
            let p = px_at(&out, xu, yu);
            let i = (yu * src.w + xu) * 4;
            out.px[i..i + 4].copy_from_slice(&pack(lerp3(rgb3(p), col, 0.85), p[3]));
            heading += curl;
            x += heading.sin();
            y += heading.cos();
        }
    }
    out
}

/// Film grain: monochrome noise of up to `amount` (0..=1) of full range, strongest in the midtones.
pub(super) fn film_grain(src: &Buf, amount: f32, seed: u64) -> Buf {
    let amount = unit(amount) * 80.0;
    let w = src.w;
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let l = luma(&p);
        let midtone = (4.0 * l * (1.0 - l)).max(0.25);
        let n = (unit_hash(seed, (y * w + x) as u64) - 0.5) * 2.0 * amount * midtone;
        pack(rgb3(p).map(|v| v + n), p[3])
    })
}

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

/// The frame of a border: `width` px of `paint(x, y, depth)` on every side of the image, where `depth`
/// is the distance from the canvas edge. The image sits unchanged in the middle.
fn frame(src: &Buf, width: u32, paint: impl Fn(usize, usize, usize) -> [u8; 4] + Sync) -> Result<Buf, EffectError> {
    let margin = width as usize;
    let (cw, ch) = grown(src.w, src.h, margin)?;
    let mut px = vec![0u8; cw * ch * 4];
    px.par_chunks_mut(cw * 4).enumerate().for_each(|(y, row)| {
        for (x, out) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let depth = x.min(y).min(cw - 1 - x).min(ch - 1 - y);
            *out = if depth < margin { paint(x, y, depth) } else { px_at(src, x - margin, y - margin) };
        }
    });
    Ok(Buf { w: cw, h: ch, px })
}

/// The canvas size with `margin` px added on every side, or `Invalid` past the pixel budget.
fn grown(w: usize, h: usize, margin: usize) -> Result<(usize, usize), EffectError> {
    let cw = w as u64 + 2 * margin as u64;
    let ch = h as u64 + 2 * margin as u64;
    if cw > u64::from(u32::MAX) || ch > u64::from(u32::MAX) || cw * ch > MAX_CANVAS_PIXELS {
        return Err(EffectError::Invalid(format!("a {cw} x {ch} canvas is past the pixel budget")));
    }
    Ok((cw as usize, ch as usize))
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

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    std::array::from_fn(|c| a[c] + (b[c] - a[c]) * t)
}

/// The colour scaled by `k`, alpha kept.
fn shade(c: Rgba, k: f32) -> [u8; 4] {
    pack(rgb_of(c).map(|v| v * k), c.a)
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

fn luma_map(src: &Buf) -> Vec<f32> {
    src.px.as_chunks::<4>().0.iter().map(|p| luma(p)).collect()
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

/// Sobel gradient magnitude of the luma, 0..=1, so a full-contrast step is 1.
fn edges(src: &Buf) -> Vec<f32> {
    let (w, h) = (src.w, src.h);
    let l = luma_map(src);
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

fn centroid(t: [[f32; 2]; 3]) -> [f32; 2] {
    [(t[0][0] + t[1][0] + t[2][0]) / 3.0, (t[0][1] + t[1][1] + t[2][1]) / 3.0]
}

/// A stable key for one facet, from its cell and which half of the cell it is.
fn facet_key(i: i64, j: i64, k: u8) -> u64 {
    (((i as u64) << 32) ^ (j as u64 & 0xffff_ffff)).wrapping_mul(2).wrapping_add(u64::from(k))
}

/// The smallest barycentric weight of `p` in triangle `t`: at least 0 when `p` is inside it.
fn inside(t: [[f32; 2]; 3], p: [f32; 2]) -> f32 {
    let [a, b, c] = t;
    let det = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
    if det.abs() < f32::EPSILON {
        return f32::NEG_INFINITY;
    }
    let l1 = ((b[1] - c[1]) * (p[0] - c[0]) + (c[0] - b[0]) * (p[1] - c[1])) / det;
    let l2 = ((c[1] - a[1]) * (p[0] - c[0]) + (a[0] - c[0]) * (p[1] - c[1])) / det;
    l1.min(l2).min(1.0 - l1 - l2)
}

/// A triangle mesh over a grid of `size` px cells, two triangles per cell. Each vertex moves up to 0.35
/// cells from its grid point, by the seed, so the facets are irregular.
struct Mesh {
    size: f32,
    seed: u64,
}

impl Mesh {
    fn vertex(&self, i: i64, j: i64) -> [f32; 2] {
        let k = (((i as u64) << 32) ^ (j as u64 & 0xffff_ffff)).wrapping_mul(2);
        let jx = (unit_hash(self.seed, k) - 0.5) * 0.7 * self.size;
        let jy = (unit_hash(self.seed, k.wrapping_add(1)) - 0.5) * 0.7 * self.size;
        [i as f32 * self.size + jx, j as f32 * self.size + jy]
    }

    fn triangle(&self, i: i64, j: i64, k: u8) -> [[f32; 2]; 3] {
        if k == 0 {
            [self.vertex(i, j), self.vertex(i + 1, j), self.vertex(i, j + 1)]
        } else {
            [self.vertex(i + 1, j), self.vertex(i + 1, j + 1), self.vertex(i, j + 1)]
        }
    }

    /// The triangle that holds `p`, as (i, j, k): the candidate in the 3x3 cells around it with the
    /// largest smallest barycentric weight.
    fn locate(&self, p: [f32; 2]) -> (i64, i64, u8) {
        let ci = (p[0] / self.size).floor() as i64;
        let cj = (p[1] / self.size).floor() as i64;
        let mut best = (ci, cj, 0u8);
        let mut best_score = f32::NEG_INFINITY;
        for i in ci - 1..=ci + 1 {
            for j in cj - 1..=cj + 1 {
                for k in 0..2u8 {
                    let score = inside(self.triangle(i, j, k), p);
                    if score > best_score {
                        best_score = score;
                        best = (i, j, k);
                    }
                }
            }
        }
        best
    }
}
