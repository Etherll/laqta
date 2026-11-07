//! Filters X4 (M7 D5): textures, prints, frames and window effects. Each kernel is written from the
//! effect's name and what it does to an image, and shares the helpers in the parent module. Pixels are
//! straight RGBA8 in and out. Every kernel is pure and deterministic for its seed.

use std::collections::BTreeMap;
use std::f32::consts::{PI, TAU};
use std::ops::RangeInclusive;

use laqta_domain::color::Rgba;
use rayon::prelude::*;

use super::{Buf, blur, cap, hash, premultiply, shifted, to_u8, unit};
use crate::effects::EffectError;

/// Largest canvas an effect may allocate, in pixels. Matches the budget in `canvas.rs`.
const MAX_CANVAS_PIXELS: u64 = 1 << 28;
/// Caps on parameters that would otherwise ask for unbounded work. Larger values render as the cap.
const MAX_CELL: u32 = 4096;
const MAX_TILES: u32 = 16;
const MAX_BANDS: u32 = 64;
const MAX_BLUR: u32 = 256;
const MAX_DROPS: u32 = 1024;
const MAX_FLAKES: u32 = 4096;
const MAX_STREAKS: u32 = 4096;
const MAX_STAR_REACH: u32 = 64;
const MAX_NEIGHBOUR: u32 = 8;
const MAX_SCATTER: u32 = 64;
const MAX_SCRATCHES: u32 = 256;
const MAX_SHIFT: u32 = 256;

/// Per-effect salts, so two effects given one seed don't draw the same numbers.
const POLAROID_SALT: u64 = 0x901D_0001;
const RUST_SALT: u64 = 0x2057_0002;
const SMOKE_SALT: u64 = 0x5300_0003;
const SMOKE_WARP_SALT: u64 = 0x5300_0004;
const SPECK_SALT: u64 = 0x5BEC_0005;
const STATIC_SALT: u64 = 0x7A71_0006;
const VHS_SALT: u64 = 0x7485_0007;
const VHS_NOISE_SALT: u64 = 0x7485_0008;

/// Perlin noise: fractal gradient noise on a lattice `scale` px apart, three octaves, mixed over the image
/// toward a grey of the same tone by `strength` (0..=1).
pub(super) fn perlin_noise(src: &Buf, scale: u32, strength: f32, seed: u64) -> Buf {
    let scale = scale.clamp(1, MAX_CELL) as f32;
    let strength = unit(strength);
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let grey = 255.0 * fbm(seed, (x as f32 + 0.5) / scale, (y as f32 + 0.5) / scale, 3);
        pack(lerp3(rgb3(p), [grey; 3], strength), p[3])
    })
}

/// Pixel sorting: in each row (or column when `vertical`), every run of pixels at least `threshold` (0..=1)
/// bright is sorted by luma, darkest first, so the bright stretches slide toward one end.
pub(super) fn pixel_sorting(src: &Buf, threshold: f32, vertical: bool) -> Buf {
    let threshold = unit(threshold);
    let (w, h) = (src.w, src.h);
    let (lines, len) = if vertical { (w, h) } else { (h, w) };
    let index = |line: usize, i: usize| if vertical { i * w + line } else { line * w + i };
    let sorted: Vec<Vec<[u8; 4]>> = (0..lines)
        .into_par_iter()
        .map(|line| {
            let mut pixels: Vec<[u8; 4]> = (0..len).map(|i| pixel(src, index(line, i))).collect();
            let bright: Vec<bool> = pixels.iter().map(|p| luma(p) >= threshold).collect();
            let mut start = None;
            for i in 0..=len {
                let is_bright = bright.get(i).copied().unwrap_or(false);
                match (start, is_bright) {
                    (None, true) => start = Some(i),
                    (Some(s), false) => {
                        pixels[s..i].sort_by(|a, b| luma(a).total_cmp(&luma(b)));
                        start = None;
                    }
                    _ => {}
                }
            }
            pixels
        })
        .collect();
    let mut out = src.clone();
    for (line, pixels) in sorted.iter().enumerate() {
        for (i, p) in pixels.iter().enumerate() {
            let k = index(line, i) * 4;
            out.px[k..k + 4].copy_from_slice(p);
        }
    }
    out
}

/// Plasma energy arcs: a sum of four sine waves makes a plasma field `scale` px across, and the arcs are the
/// places where it crosses zero. Those arcs are mixed toward `color` by `strength` (0..=1).
pub(super) fn plasma_energy_arcs(src: &Buf, scale: u32, strength: f32, color: Rgba, seed: u64) -> Buf {
    let scale = scale.clamp(1, MAX_CELL) as f32;
    let strength = unit(strength);
    let target = rgb_of(color);
    let phase: [f32; 4] = std::array::from_fn(|k| TAU * unit_hash(seed, k as u64));
    per_pixel(src, |x, y| {
        let (fx, fy) = ((x as f32 + 0.5) / scale, (y as f32 + 0.5) / scale);
        let field = (2.0 * fx + phase[0]).sin()
            + (3.0 * fy + phase[1]).sin()
            + ((fx + fy) * 1.5 + phase[2]).sin()
            + (2.0 * fx.hypot(fy) + phase[3]).sin();
        // Brightest on the zero contour, fading out over |field| < 0.45.
        let arc = (1.0 - field.abs() / 0.45).clamp(0.0, 1.0).powi(2);
        let p = px_at(src, x, y);
        pack(lerp3(rgb3(p), target, strength * arc), p[3])
    })
}

/// Pointillism: the image rebuilt from dots. Each `cell` px square holds one dot at a seeded spot, painted in
/// the colour of the image under it. Between the dots the paper stays white.
pub(super) fn pointillism(src: &Buf, cell: u32, seed: u64) -> Buf {
    let s = cell.clamp(1, MAX_CELL);
    let size = s as f32;
    let cell_px = s as usize;
    // Centre and radius of the dot in cell (`i`, `j`).
    let dot = |i: i64, j: i64| {
        let (ox, oy, r) = (cell_unit(seed, i, j, 1), cell_unit(seed, i, j, 2), cell_unit(seed, i, j, 3));
        let centre = [(i as f32 + 0.2 + 0.6 * ox) * size, (j as f32 + 0.2 + 0.6 * oy) * size];
        (centre, size * (0.3 + 0.12 * r))
    };
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let (cx, cy) = ((x / cell_px) as i64, (y / cell_px) as i64);
        // The nearest dot that covers the pixel: (distance over radius, colour).
        let mut best = (f32::INFINITY, [255.0f32; 3]);
        for j in cy - 1..=cy + 1 {
            for i in cx - 1..=cx + 1 {
                let (c, r) = dot(i, j);
                let d = (fx - c[0]).hypot(fy - c[1]) / r;
                if d <= 1.0 && d < best.0 {
                    best = (d, rgb3(sample(src, c)));
                }
            }
        }
        pack(best.1, p[3])
    })
}

/// Polaroid frame: the image on a `color` paper mount, `border` px wide on three sides and `bottom` px deeper
/// underneath. The paper darkens a shade where it meets the photo, and carries a little grain.
pub(super) fn polaroid_frame(src: &Buf, border: u32, bottom: u32, color: Rgba) -> Result<Buf, EffectError> {
    let side = border as usize;
    let below = side + bottom as usize;
    let paper = rgb_of(color);
    let (iw, ih) = (src.w as i64, src.h as i64);
    place(src, [side, side, side, below], |x, y| {
        let (ix, iy) = (x as i64 - side as i64, y as i64 - side as i64);
        // Steps from the photo: 1 for the paper touching it.
        let gap = (-ix).max(ix - (iw - 1)).max(0) + (-iy).max(iy - (ih - 1)).max(0);
        let shade = if gap == 1 { 0.85 } else { 1.0 };
        let grain = 0.97 + 0.03 * unit_hash(POLAROID_SALT, (y * 65_536 + x) as u64);
        pack(paper.map(|c| c * shade * grain), 255)
    })
}

/// Pop art grid: the image cut into `tiles` x `tiles` panels, each a duotone from a deep violet shadow to one
/// bright ink in turn, with a one-px white gutter between the panels.
pub(super) fn pop_art_grid(src: &Buf, tiles: u32) -> Buf {
    const INKS: [[f32; 3]; 6] = [
        [255.0, 64.0, 160.0],
        [255.0, 222.0, 40.0],
        [40.0, 200.0, 230.0],
        [255.0, 128.0, 40.0],
        [90.0, 210.0, 90.0],
        [160.0, 100.0, 230.0],
    ];
    const SHADOW: [f32; 3] = [30.0, 20.0, 50.0];
    let n = tiles.clamp(1, MAX_TILES) as usize;
    let (w, h) = (src.w, src.h);
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let gutter = n > 1 && ((x > 0 && (x * n) % w < n) || (y > 0 && (y * n) % h < n));
        if gutter {
            return [255, 255, 255, p[3]];
        }
        let (tx, ty) = (x * n / w, y * n / h);
        let ink = INKS[(ty * n + tx) % INKS.len()];
        pack(lerp3(SHADOW, ink, luma(&p)), p[3])
    })
}

/// Radial rainbow: the hue turns `bands` times around the centre. Each pixel takes that hue at its own
/// brightness, mixed in by `strength` (0..=1).
pub(super) fn radial_rainbow(src: &Buf, strength: f32, bands: u32) -> Buf {
    let strength = unit(strength);
    let bands = bands.clamp(1, MAX_BANDS) as f32;
    let (cx, cy) = (src.w as f32 / 2.0, src.h as f32 / 2.0);
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
        let hue = (dy.atan2(dx) / TAU + 0.5) * bands;
        let colour = hsl_rgb(hue, 0.9, 0.25 + 0.5 * luma(&p));
        pack(lerp3(rgb3(p), colour, strength), p[3])
    })
}

/// Rainbow border: a `width` px frame whose hue turns once around the image, a little darker toward the
/// outer edge.
pub(super) fn rainbow_border(src: &Buf, width: u32) -> Result<Buf, EffectError> {
    let (cx, cy) = (src.w as f32 / 2.0 + width as f32, src.h as f32 / 2.0 + width as f32);
    let m = width.max(1) as f32;
    frame(src, width, |x, y, depth| {
        let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
        let hue = dy.atan2(dx) / TAU + 0.5;
        let shade = 0.6 + 0.4 * (depth as f32 + 0.5) / m;
        pack(hsl_rgb(hue, 0.95, 0.5).map(|c| c * shade), 255)
    })
}

/// Rainy window: the image blurred by `blur_radius` px, with `drops` water drops on it. Each drop, up to
/// `size` px across, shows the sharp image bent through its lens, with a bright rim.
pub(super) fn rainy_window(src: &Buf, drops: u32, size: u32, blur_radius: u32, seed: u64) -> Buf {
    let mut out = blur::box_blur(src, blur_radius.min(MAX_BLUR));
    let (w, h) = (src.w, src.h);
    let largest = size.clamp(1, MAX_CELL) as f32;
    for k in 0..u64::from(drops.min(MAX_DROPS)) {
        let (cx, cy) = (unit_hash(seed, 4 * k) * w as f32, unit_hash(seed, 4 * k + 1) * h as f32);
        let r = largest * (0.5 + 0.5 * unit_hash(seed, 4 * k + 2));
        for y in reach(cy, r, h) {
            for x in reach(cx, r, w) {
                let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
                let d = dx.hypot(dy);
                if d > r {
                    continue;
                }
                let p = sample(src, [cx + dx * 0.6, cy + dy * 0.6]);
                let rim = smoothstep(r - 1.5, r, d);
                let c = pack(lerp3(rgb3(p), [255.0; 3], 0.35 * rim), p[3]);
                let i = (y * w + x) * 4;
                out.px[i..i + 4].copy_from_slice(&c);
            }
        }
    }
    out
}

/// Retro sunset: the image mapped through a dusk gradient, from violet shadows through magenta and orange to
/// pale yellow, mixed in by `strength` (0..=1). Every third row in the lower half is darkened, as scan bars.
pub(super) fn retro_sunset(src: &Buf, strength: f32) -> Buf {
    const DUSK: [(f32, [f32; 3]); 4] =
        [(0.0, [20.0, 0.0, 50.0]), (0.35, [150.0, 20.0, 110.0]), (0.7, [255.0, 100.0, 80.0]), (1.0, [255.0, 225.0, 140.0])];
    let strength = unit(strength);
    let h = src.h;
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let mut c = lerp3(rgb3(p), gradient(&DUSK, luma(&p)), strength);
        if y >= h / 2 && y.is_multiple_of(3) {
            c = c.map(|v| v * 0.55);
        }
        pack(c, p[3])
    })
}

/// Risograph print: the image printed in one `ink` on `paper`. Ink builds where the image is dark, broken up
/// by `grain` (0..=1) of random dropout, and the ink plate sits one px off the paper.
pub(super) fn riso_print(src: &Buf, ink: Rgba, paper: Rgba, grain: f32, seed: u64) -> Buf {
    let grain = unit(grain);
    let (ink, paper) = (rgb_of(ink), rgb_of(paper));
    let (w, lum) = (src.w, luma_map(src));
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let coverage = 1.0 - lum[y * w + x.saturating_sub(1)];
        let threshold = 0.5 + (unit_hash(seed, (y * w + x) as u64) - 0.5) * grain;
        let t = smoothstep(threshold - 0.05, threshold + 0.05, coverage);
        pack(lerp3(paper, ink, t), p[3])
    })
}

/// Rope border: a `width` px frame of twisted rope. Diagonal strands alternate between `color` and `accent`,
/// each lit across its middle like a cylinder.
pub(super) fn rope_border(src: &Buf, width: u32, color: Rgba, accent: Rgba) -> Result<Buf, EffectError> {
    const STRAND: usize = 8;
    let (strong, weak) = (rgb_of(color), rgb_of(accent));
    let m = width.max(1) as f32;
    frame(src, width, |x, y, depth| {
        let along = x + y;
        let f = (along % STRAND) as f32 / STRAND as f32;
        let lit = 0.45 + 0.55 * (PI * f).sin();
        let edge = 0.8 + 0.2 * (depth as f32 + 0.5) / m;
        let base = if (along / STRAND).is_multiple_of(2) { strong } else { weak };
        pack(base.map(|c| c * lit * edge), 255)
    })
}

/// Rust corrosion: rust-brown patches where fractal noise runs high, `scale` px across. Inside a patch the
/// image dulls toward rust, with fine speckle, by `amount` (0..=1).
pub(super) fn rust_corrosion(src: &Buf, amount: f32, scale: u32, seed: u64) -> Buf {
    const RUST: [f32; 3] = [150.0, 70.0, 30.0];
    let amount = unit(amount);
    let scale = scale.clamp(1, MAX_CELL) as f32;
    let w = src.w;
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let n = fbm(seed, (x as f32 + 0.5) / scale, (y as f32 + 0.5) / scale, 4);
        let patch = smoothstep(0.55, 0.7, n) * amount;
        let speckle = 0.8 + 0.4 * unit_hash(seed ^ RUST_SALT, (y * w + x) as u64);
        let rust = RUST.map(|c| c * speckle * (0.5 + 0.7 * luma(&p)));
        pack(lerp3(rgb3(p), rust, patch * 0.9), p[3])
    })
}

/// Scanline: every `spacing`-th row is darkened by `strength` (0..=1), as on an old monitor.
pub(super) fn scanline(src: &Buf, spacing: u32, strength: f32) -> Buf {
    let every = spacing.clamp(1, MAX_CELL) as usize;
    let dim = 1.0 - unit(strength);
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let k = if y.is_multiple_of(every) { dim } else { 1.0 };
        pack(rgb3(p).map(|c| c * k), p[3])
    })
}

/// Smoke overlay: wisps of grey smoke, fractal noise `scale` px across, with a second noise field warping the
/// lookup so the wisps curl. Screened over the image at `opacity` (0..=1).
pub(super) fn smoke_overlay(src: &Buf, opacity: f32, scale: u32, seed: u64) -> Buf {
    let opacity = unit(opacity);
    let scale = scale.clamp(1, MAX_CELL) as f32;
    per_pixel(src, |x, y| {
        let (fx, fy) = ((x as f32 + 0.5) / scale, (y as f32 + 0.5) / scale);
        let warp_x = fbm(seed ^ SMOKE_SALT, fx, fy, 3) - 0.5;
        let warp_y = fbm(seed ^ SMOKE_WARP_SALT, fx, fy, 3) - 0.5;
        let wisp = fbm(seed, fx + 4.0 * warp_x, fy + 4.0 * warp_y, 4);
        let s = smoothstep(0.5, 0.75, wisp) * opacity;
        let p = px_at(src, x, y);
        pack(rgb3(p).map(|c| c + (255.0 - c) * s), p[3])
    })
}

/// Snowfall with depth fog: haze thickens toward the top of the image and lightens it by `fog` (0..=1), and
/// `flakes` white flakes of one or two px fall across it, a third of them the larger size.
pub(super) fn snowfall_depth_fog(src: &Buf, flakes: u32, fog: f32, seed: u64) -> Buf {
    const HAZE: [f32; 3] = [226.0, 232.0, 240.0];
    let fog = unit(fog);
    let (w, h) = (src.w, src.h);
    let mut out = per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let far = 1.0 - (y as f32 + 0.5) / h as f32;
        pack(lerp3(rgb3(p), HAZE, fog * far * far), p[3])
    });
    for k in 0..u64::from(flakes.min(MAX_FLAKES)) {
        let (fx, fy) = (unit_hash(seed, 3 * k) * w as f32, unit_hash(seed, 3 * k + 1) * h as f32);
        let radius = if unit_hash(seed, 3 * k + 2) < 0.3 { 2.0 } else { 1.0 };
        for y in reach(fy, radius, h) {
            for x in reach(fx, radius, w) {
                let (dx, dy) = (x as f32 + 0.5 - fx, y as f32 + 0.5 - fy);
                if dx.hypot(dy) <= radius {
                    let i = (y * w + x) * 4;
                    out.px[i..i + 3].fill(255);
                }
            }
        }
    }
    out
}

/// Sobel edges: the gradient strength of the luma, drawn as light on black. `gain` scales it, so a
/// full-contrast edge is white at gain 1.
pub(super) fn sobel_edge(src: &Buf, gain: f32) -> Buf {
    let gain = cap(gain, 64.0).max(0.0);
    let mag = sobel(src);
    let w = src.w;
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let e = (mag[y * w + x] * gain).clamp(0.0, 1.0);
        pack([255.0 * e; 3], p[3])
    })
}

/// Soft diffusion: each pixel scatters to a seeded spot up to `radius` px away, and the colour found there is
/// mixed in by `strength` (0..=1). Edges soften into a haze without an even blur.
pub(super) fn soft_diffusion(src: &Buf, radius: u32, strength: f32, seed: u64) -> Buf {
    let r = radius.min(MAX_SCATTER) as i64;
    if r == 0 {
        return src.clone();
    }
    let (w, h) = (src.w, src.h);
    let strength = unit(strength);
    let span = (2 * r + 1) as f32;
    per_pixel(src, |x, y| {
        let i = (y * w + x) as u64;
        let dx = ((unit_hash(seed, 2 * i) * span) as i64).min(2 * r) - r;
        let dy = ((unit_hash(seed, 2 * i + 1) * span) as i64).min(2 * r) - r;
        let scattered = px_at(src, shifted(x, -dx, w), shifted(y, -dy, h));
        let p = px_at(src, x, y);
        pack(lerp3(rgb3(p), rgb3(scattered), strength), p[3])
    })
}

/// Spin blur: a rotational blur about a centre, given as fractions of the image size. Each pixel averages
/// samples along an arc `angle` degrees wide (at most 180) around that centre.
pub(super) fn spin_blur(src: &Buf, angle: f32, center_x: f32, center_y: f32) -> Buf {
    const SAMPLES: usize = 24;
    let arc = cap(angle, 180.0).max(0.0).to_radians();
    let (w, h) = (src.w, src.h);
    let (cx, cy) = (unit(center_x) * w as f32, unit(center_y) * h as f32);
    let pm = premultiply(&src.px);
    per_pixel(src, |x, y| {
        let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
        let (dist, base) = (dx.hypot(dy), dy.atan2(dx));
        let mut acc = [0.0f32; 4];
        for s in 0..SAMPLES {
            let t = s as f32 / (SAMPLES - 1) as f32 - 0.5;
            let a = base + arc * t;
            let (sx, sy) = (cx + dist * a.cos(), cy + dist * a.sin());
            let i = (clamp_index(sy, h) * w + clamp_index(sx, w)) * 4;
            for (sum, v) in acc.iter_mut().zip(&pm[i..i + 4]) {
                *sum += *v;
            }
        }
        let n = SAMPLES as f32;
        let alpha = acc[3] / n;
        if alpha <= 0.0 {
            return [0, 0, 0, 0];
        }
        let rgb = [acc[0], acc[1], acc[2]].map(|c| c / n / alpha * 255.0);
        pack(rgb, to_u8(alpha * 255.0))
    })
}

/// Stained glass: the image as panes of glass, one per Voronoi cell about `cell` px across, each the cell's
/// average colour. Lead lines `lead` px wide run along the cell borders.
pub(super) fn stained_glass(src: &Buf, cell: u32, lead: u32, seed: u64) -> Buf {
    const LEAD: [f32; 3] = [25.0, 20.0, 20.0];
    let size = cell.clamp(1, MAX_CELL) as f32;
    let (w, h) = (src.w, src.h);
    let hits: Vec<VoronoiHit> =
        (0..w * h).into_par_iter().map(|k| voronoi_nearest(seed, size, [(k % w) as f32 + 0.5, (k / w) as f32 + 0.5])).collect();
    // The colour sum and pixel count of each cell, in cell order.
    let mut sums: BTreeMap<(i64, i64), ([f32; 3], f32)> = BTreeMap::new();
    for (k, hit) in hits.iter().enumerate() {
        let entry = sums.entry(hit.cell).or_insert(([0.0; 3], 0.0));
        for (acc, v) in entry.0.iter_mut().zip(rgb3(pixel(src, k))) {
            *acc += v;
        }
        entry.1 += 1.0;
    }
    let lead = lead as f32;
    per_pixel(src, |x, y| {
        let hit = &hits[y * w + x];
        let p = px_at(src, x, y);
        if hit.second - hit.dist < lead {
            return pack(LEAD, p[3]);
        }
        let (sum, n) = sums[&hit.cell];
        pack(sum.map(|v| v / n), p[3])
    })
}

/// Stained glass border: a `width` px frame of glass panes, `pane` px square, each a jewel tone from the seed,
/// with `lead` lines on the grid between them.
pub(super) fn stained_glass_border(src: &Buf, width: u32, pane: u32, lead: Rgba, seed: u64) -> Result<Buf, EffectError> {
    let pane = pane.clamp(2, MAX_CELL) as usize;
    let lead = rgba4(lead);
    frame(src, width, |x, y, _| {
        if x.is_multiple_of(pane) || y.is_multiple_of(pane) {
            return lead;
        }
        let hue = cell_unit(seed, (x / pane) as i64, (y / pane) as i64, 6);
        pack(hsl_rgb(hue, 0.7, 0.45), 255)
    })
}

/// Starfield border: a `width` px frame of `background` night sky, with `star` pinpricks scattered from the seed.
pub(super) fn starfield_border(src: &Buf, width: u32, background: Rgba, star: Rgba, seed: u64) -> Result<Buf, EffectError> {
    let (sky, bright) = (rgba4(background), rgba4(star));
    frame(src, width, |x, y, _| if unit_hash(seed, ((y as u64) << 32) | x as u64) < 0.03 { bright } else { sky })
}

/// Star filter: pixels brighter than `threshold` (0..=1 luma) throw four glints, horizontal, vertical and both
/// diagonals, up to `length` px long (capped at 64), tinted `color` and added at `strength`.
pub(super) fn star_filter(src: &Buf, threshold: f32, length: u32, strength: f32, color: Rgba) -> Buf {
    let (w, h) = (src.w, src.h);
    let threshold = unit(threshold).min(0.999);
    let reach_px = length.min(MAX_STAR_REACH) as i64;
    let gain = unit(strength);
    let tint = rgb_of(color).map(|c| c / 255.0);
    let bright: Vec<f32> = luma_map(src).into_iter().map(|l| ((l - threshold) / (1.0 - threshold)).clamp(0.0, 1.0)).collect();
    per_pixel(src, |x, y| {
        let mut glint = 0.0f32;
        for (dx, dy) in [(1i64, 0i64), (0, 1), (1, 1), (1, -1)] {
            for k in 1..=reach_px {
                // Each glint fades with distance, and runs out both ways along its ray.
                let fade = (1.0 - k as f32 / (reach_px as f32 + 1.0)).powi(2);
                for sign in [-1i64, 1] {
                    let (sx, sy) = (x as i64 + sign * dx * k, y as i64 + sign * dy * k);
                    if (0..w as i64).contains(&sx) && (0..h as i64).contains(&sy) {
                        glint += bright[sy as usize * w + sx as usize] * fade;
                    }
                }
            }
        }
        let energy = glint * 0.5 / reach_px.max(1) as f32;
        let p = px_at(src, x, y);
        pack(std::array::from_fn(|c| rgb3(p)[c] + 255.0 * gain * tint[c] * energy), p[3])
    })
}

/// Stripe overlay: diagonal stripes `period` px apart, each covering half a period, tinted `color` at
/// `opacity` (0..=1).
pub(super) fn stripe_overlay(src: &Buf, period: u32, opacity: f32, color: Rgba) -> Buf {
    let period = period.clamp(2, MAX_CELL) as usize;
    let opacity = unit(opacity);
    let target = rgb_of(color);
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        if (x + y) % period < period / 2 { pack(lerp3(rgb3(p), target, opacity), p[3]) } else { p }
    })
}

/// Surface blur: each pixel averages the `(2 * radius + 1)` square around it, but only the neighbours whose
/// luma is within `threshold` (0..=255) of its own. Strong edges hold and smooth areas blur. The radius is
/// capped at 8.
pub(super) fn surface_blur(src: &Buf, radius: u32, threshold: u8) -> Buf {
    let r = radius.min(MAX_NEIGHBOUR) as usize;
    let t = f32::from(threshold);
    let (w, h) = (src.w, src.h);
    per_pixel(src, |x, y| {
        let c = px_at(src, x, y);
        let centre = luma(&c) * 255.0;
        let mut sum = [0u32; 3];
        let mut n = 0u32;
        for ny in y.saturating_sub(r)..=(y + r).min(h - 1) {
            for nx in x.saturating_sub(r)..=(x + r).min(w - 1) {
                let q = px_at(src, nx, ny);
                if (luma(&q) * 255.0 - centre).abs() <= t {
                    for (s, v) in sum.iter_mut().zip(q) {
                        *s += u32::from(v);
                    }
                    n += 1;
                }
            }
        }
        // The centre always counts, so `n` is at least 1.
        pack(sum.map(|s| s as f32 / n as f32), c[3])
    })
}

/// Thermal vision: luma mapped through a heat palette, from violet cold through red and yellow to white hot,
/// mixed in by `strength` (0..=1).
pub(super) fn thermal_vision(src: &Buf, strength: f32) -> Buf {
    const HEAT: [(f32, [f32; 3]); 6] = [
        (0.0, [10.0, 0.0, 40.0]),
        (0.2, [80.0, 0.0, 130.0]),
        (0.45, [210.0, 20.0, 60.0]),
        (0.7, [255.0, 140.0, 0.0]),
        (0.9, [255.0, 240.0, 80.0]),
        (1.0, [255.0, 255.0, 255.0]),
    ];
    let strength = unit(strength);
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let heat = gradient(&HEAT, luma(&p));
        pack(lerp3(rgb3(p), heat, strength), p[3])
    })
}

/// Tilt shift: a miniature look. The rows within `band` (0..=1 of the height) either side of the `focus` row
/// (0..=1 down the image) stay sharp, and the blur of `radius` px fades in above and below them.
pub(super) fn tilt_shift(src: &Buf, focus: f32, band: f32, radius: u32) -> Buf {
    let soft = blur::box_blur(src, radius.min(MAX_BLUR));
    let (focus, band) = (unit(focus), unit(band));
    let h = src.h as f32;
    per_pixel(src, |x, y| {
        let distance = ((y as f32 + 0.5) / h - focus).abs();
        let k = smoothstep(band, band + 0.25, distance);
        let p = px_at(src, x, y);
        let q = px_at(&soft, x, y);
        pack(lerp3(rgb3(p), rgb3(q), k), p[3])
    })
}

/// Triangle pixelate: square cells `size` px across, each split along its diagonal, with each half filled
/// with its own average colour. The cell size is at least 2.
pub(super) fn triangle_pixelate(src: &Buf, size: u32) -> Buf {
    let s = size.clamp(2, MAX_CELL) as usize;
    let (w, h) = (src.w, src.h);
    let (cols, rows) = (w.div_ceil(s), h.div_ceil(s));
    // Per cell, the average colour of the half right of the diagonal (0) and the half left of it (1).
    let halves: Vec<[[u8; 3]; 2]> = (0..cols * rows)
        .into_par_iter()
        .map(|c| {
            let (x0, y0) = ((c % cols) * s, (c / cols) * s);
            let mut sums = [[0u64; 4]; 2];
            for y in y0..(y0 + s).min(h) {
                for x in x0..(x0 + s).min(w) {
                    let p = px_at(src, x, y);
                    let acc = &mut sums[usize::from(x - x0 > y - y0)];
                    acc[0] += u64::from(p[0]);
                    acc[1] += u64::from(p[1]);
                    acc[2] += u64::from(p[2]);
                    acc[3] += 1;
                }
            }
            sums.map(|[r, g, b, n]| {
                let n = n.max(1);
                [r, g, b].map(|v| (v / n) as u8)
            })
        })
        .collect();
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let half = usize::from(x % s > y % s);
        let avg = halves[(y / s) * cols + x / s][half];
        pack(avg.map(f32::from), p[3])
    })
}

/// TV static: `amount` (0..=1) of the pixels turn to random grey snow, and the rest keep the image.
pub(super) fn tv_static(src: &Buf, amount: f32, seed: u64) -> Buf {
    let amount = unit(amount);
    let w = src.w;
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let i = (y * w + x) as u64;
        if unit_hash(seed, i) < amount {
            let grey = to_u8(255.0 * unit_hash(seed ^ STATIC_SALT, i));
            [grey, grey, grey, p[3]]
        } else {
            p
        }
    })
}

/// Unsharp mask: each channel gets `amount` (0..=8) times its difference from a box blur `radius` px wide,
/// added back only where that difference is at least `threshold`, so flat areas stay smooth.
pub(super) fn unsharp_mask(src: &Buf, radius: u32, amount: f32, threshold: u8) -> Buf {
    let soft = blur::box_blur(src, radius.min(MAX_BLUR));
    let gain = cap(amount, 8.0).max(0.0);
    let threshold = f32::from(threshold);
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let q = px_at(&soft, x, y);
        let sharpened = std::array::from_fn(|c| {
            let v = f32::from(p[c]);
            let detail = v - f32::from(q[c]);
            if detail.abs() >= threshold { v + gain * detail } else { v }
        });
        pack(sharpened, p[3])
    })
}

/// VHS tape damage: a fifth of the rows jitter sideways by up to `jitter` px, red smears left and blue smears
/// right by `bleed` px, and the bottom eighth of the image, where the tracking has slipped, fills with static
/// mixed in by `noise` (0..=1).
pub(super) fn vhs_tape_damage(src: &Buf, jitter: u32, bleed: u32, noise: f32, seed: u64) -> Buf {
    let (w, h) = (src.w, src.h);
    let jitter = i64::from(jitter.min(MAX_SHIFT));
    let bleed = i64::from(bleed.min(MAX_SHIFT));
    let noise = unit(noise);
    let band = (h / 8).max(1);
    per_pixel(src, |x, y| {
        let row = y as u64;
        let shift = if unit_hash(seed, row) < 0.2 {
            ((unit_hash(seed ^ VHS_SALT, row) * (2 * jitter + 1) as f32) as i64 - jitter).clamp(-jitter, jitter)
        } else {
            0
        };
        let sx = shifted(x, -shift, w);
        let p = px_at(src, sx, y);
        let red = px_at(src, shifted(sx, bleed, w), y)[0];
        let blue = px_at(src, shifted(sx, -bleed, w), y)[2];
        let mut c = [f32::from(red), f32::from(p[1]), f32::from(blue)];
        if y + band >= h && noise > 0.0 {
            let grey = 255.0 * unit_hash(seed ^ VHS_NOISE_SALT, (y * w + x) as u64);
            c = lerp3(c, [grey; 3], noise);
        }
        pack(c, p[3])
    })
}

/// Vintage print damage: the print fades toward a warm sepia tone by `fade` (0..=1), with `scratches` thin
/// light scratches running down it and dust specks scattered over it.
pub(super) fn vintage_print_damage(src: &Buf, fade: f32, scratches: u32, seed: u64) -> Buf {
    const INK: [f32; 3] = [70.0, 45.0, 25.0];
    const PAPER: [f32; 3] = [245.0, 228.0, 190.0];
    let fade = unit(fade);
    let (w, h) = (src.w, src.h);
    let mut out = per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        if unit_hash(seed ^ SPECK_SALT, (y * w + x) as u64) < 0.01 {
            return [60, 40, 25, p[3]];
        }
        let toned = lerp3(INK, PAPER, luma(&p));
        pack(lerp3(rgb3(p), toned, fade), p[3])
    });
    for k in 0..u64::from(scratches.min(MAX_SCRATCHES)) {
        let x = clamp_index(unit_hash(seed, 2 * k) * w as f32, w);
        let len = (h as f32 * (0.4 + 0.6 * unit_hash(seed, 2 * k + 1))) as usize;
        for y in 0..len.min(h) {
            whiten(&mut out, x, y, 0.8);
        }
    }
    out
}

/// Voronoi mosaic: each pixel takes the colour under its cell's feature point, so the image becomes a
/// patchwork of irregular cells about `cell` px across.
pub(super) fn voronoi_mosaic(src: &Buf, cell: u32, seed: u64) -> Buf {
    let size = cell.clamp(1, MAX_CELL) as f32;
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let hit = voronoi_nearest(seed, size, [x as f32 + 0.5, y as f32 + 0.5]);
        pack(rgb3(sample(src, hit.point)), p[3])
    })
}

/// Watercolor border: a `width` px frame of `color` wash, heavier in patches where fractal noise runs high,
/// with a darker pigment line along the inner edge where the wash meets the picture.
pub(super) fn watercolor_border(src: &Buf, width: u32, color: Rgba, seed: u64) -> Result<Buf, EffectError> {
    let target = rgb_of(color);
    let inner = width as f32 - 1.0;
    frame(src, width, |x, y, depth| {
        let density = (0.3 + 0.7 * fbm(seed, x as f32 / 6.0, y as f32 / 6.0, 4)).clamp(0.0, 1.0);
        let pigment = if depth as f32 >= inner { 0.75 } else { 1.0 };
        pack(lerp3([255.0; 3], target.map(|c| c * pigment), density), 255)
    })
}

/// Watercolor Kuwahara: each pixel takes the mean of whichever of four `radius` px quadrants around it is
/// most even in luma, which gives flat painted regions with soft edges between them.
pub(super) fn watercolor_kuwahara(src: &Buf, radius: u32) -> Buf {
    let r = radius.min(MAX_NEIGHBOUR) as i64;
    if r == 0 {
        return src.clone();
    }
    let (w, h) = (src.w as i64, src.h as i64);
    let lum = luma_map(src);
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let mut best = (f32::INFINITY, [0.0f32; 3]);
        for (qx, qy) in [(-1i64, -1i64), (1, -1), (-1, 1), (1, 1)] {
            let (mut n, mut sum, mut sq, mut rgb) = (0.0f32, 0.0f32, 0.0f32, [0.0f32; 3]);
            for dy in 0..=r {
                for dx in 0..=r {
                    let sx = (x as i64 + qx * dx).clamp(0, w - 1) as usize;
                    let sy = (y as i64 + qy * dy).clamp(0, h - 1) as usize;
                    let l = lum[sy * src.w + sx];
                    n += 1.0;
                    sum += l;
                    sq += l * l;
                    for (acc, v) in rgb.iter_mut().zip(rgb3(px_at(src, sx, sy))) {
                        *acc += v;
                    }
                }
            }
            let mean = sum / n;
            let variance = sq / n - mean * mean;
            if variance < best.0 {
                best = (variance, rgb.map(|v| v / n));
            }
        }
        pack(best.1, p[3])
    })
}

/// Wet plate collodion: a blue-grey monochrome print. Tones are pushed toward high contrast, the emulsion has
/// faint mottling, and the corners fall into shadow by `vignette` (0..=1).
pub(super) fn wet_plate_collodion(src: &Buf, vignette: f32, seed: u64) -> Buf {
    const TONE: [f32; 3] = [0.8, 0.9, 1.0];
    let vignette = unit(vignette);
    let (w, h) = (src.w, src.h);
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let half = cx.hypot(cy).max(f32::MIN_POSITIVE);
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let contrast = smoothstep(0.0, 1.0, luma(&p));
        let mottle = 0.92 + 0.16 * fbm(seed, x as f32 / 5.0, y as f32 / 5.0, 2);
        let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
        let falloff = 1.0 - vignette * smoothstep(0.5, 1.2, dx.hypot(dy) / half);
        let k = contrast * mottle * falloff * 255.0;
        pack(TONE.map(|t| t * k), p[3])
    })
}

/// Windows 11 window: the image with rounded corners `radius` px across and a thin light edge, on a soft
/// shadow `shadow` px wide. The canvas grows by `shadow` on every side, and the corners stay transparent.
pub(super) fn windows_11(src: &Buf, radius: u32, shadow: u32) -> Result<Buf, EffectError> {
    let s = shadow as usize;
    let (cw, ch) = grown(src.w, src.h, [s; 4])?;
    let (w, h) = (src.w as f32, src.h as f32);
    let half = [w / 2.0, h / 2.0];
    let centre = [s as f32 + w / 2.0, s as f32 + h / 2.0];
    let corner = (radius as f32).min(w.min(h) / 2.0);
    let offset = s as f32 / 4.0;
    let fall = s as f32;
    let (iw, ih) = (src.w as i64, src.h as i64);
    let mut px = vec![0u8; cw * ch * 4];
    px.par_chunks_mut(cw * 4).enumerate().for_each(|(y, row)| {
        for (x, out) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let point = [x as f32 + 0.5, y as f32 + 0.5];
            let d = round_box(point, centre, half, corner);
            let cover = (0.5 - d).clamp(0.0, 1.0);
            // The shadow is the window's outline dropped by `drop` px and faded over `fall` px.
            let shade = 0.4 * (1.0 - smoothstep(0.0, fall, round_box(point, [centre[0], centre[1] + offset], half, corner)));
            let (ix, iy) = (x as i64 - s as i64, y as i64 - s as i64);
            let (rgb, covered) = if cover > 0.0 && (0..iw).contains(&ix) && (0..ih).contains(&iy) {
                let p = px_at(src, ix as usize, iy as usize);
                // A light rim, one px inside the edge.
                let rim = (1.0 + d).clamp(0.0, 1.0);
                (lerp3(rgb3(p), [240.0; 3], 0.6 * rim), cover * f32::from(p[3]) / 255.0)
            } else {
                ([0.0; 3], 0.0)
            };
            let alpha = covered + shade * (1.0 - covered);
            *out = if alpha <= 0.0 { [0, 0, 0, 0] } else { pack(rgb.map(|v| v * covered / alpha), to_u8(alpha * 255.0)) };
        }
    });
    Ok(Buf { w: cw, h: ch, px })
}

/// Wind streaks: `streaks` pale horizontal streaks, up to `length` px long, blown across the image at
/// `strength` (0..=1). Each is two px thick and fades toward its tail.
pub(super) fn wind_streaks(src: &Buf, streaks: u32, length: u32, strength: f32, seed: u64) -> Buf {
    let (w, h) = (src.w, src.h);
    let gain = unit(strength);
    let longest = length.clamp(1, MAX_CELL) as f32;
    let mut out = src.clone();
    for k in 0..u64::from(streaks.min(MAX_STREAKS)) {
        let x0 = clamp_index(unit_hash(seed, 4 * k) * w as f32, w);
        let y0 = clamp_index(unit_hash(seed, 4 * k + 1) * h as f32, h);
        let len = (longest * (0.5 + 0.5 * unit_hash(seed, 4 * k + 2))).max(1.0) as usize;
        for i in 0..len.min(w - x0) {
            let t = gain * (1.0 - i as f32 / len as f32);
            whiten(&mut out, x0 + i, y0, t);
            if y0 + 1 < h {
                whiten(&mut out, x0 + i, y0 + 1, t);
            }
        }
    }
    out
}

/// Woodcut print: black ink cut into paper. Each diagonal gouge `line` px wide is inked where its own width
/// reaches the image's tone, so darker tones keep more of each gouge.
pub(super) fn woodcut_print(src: &Buf, line: u32, ink: Rgba, paper: Rgba) -> Buf {
    let period = line.clamp(2, MAX_CELL) as usize;
    let (ink, paper) = (rgb_of(ink), rgb_of(paper));
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        // Across the diagonal gouge: 0 at its edges, 1 in its middle.
        let phase = ((x + y) % period) as f32 / period as f32;
        let gouge = 1.0 - (2.0 * phase - 1.0).abs();
        let cover = smoothstep(luma(&p) - 0.08, luma(&p) + 0.08, gouge);
        pack(lerp3(paper, ink, cover), p[3])
    })
}

/// Wooden frame: a `width` px frame of planks in `color`, with grain that wanders with the seed and a bevel,
/// darker on the outer edge and lighter on the inner.
pub(super) fn wooden_frame(src: &Buf, width: u32, color: Rgba, seed: u64) -> Result<Buf, EffectError> {
    let base = rgb_of(color);
    let inner = width.max(1) as f32 - 1.0;
    frame(src, width, |x, y, depth| {
        let wander = fbm(seed, (x + y) as f32 / 24.0, depth as f32 / 6.0, 3);
        let grain = 0.85 + 0.15 * (PI * (0.8 * depth as f32 + 5.0 * wander)).sin();
        let bevel = if depth == 0 {
            0.6
        } else if depth as f32 >= inner {
            1.15
        } else {
            1.0
        };
        pack(base.map(|c| c * grain * bevel), 255)
    })
}

/// X-ray scan: the image as an X-ray plate, with the luma inverted and tinted `tint` on black. A bright beam
/// crosses the image at `position` (0..=1 down the height).
pub(super) fn xray_scan(src: &Buf, position: f32, tint: Rgba) -> Buf {
    let position = unit(position);
    let tint = rgb_of(tint).map(|c| c / 255.0);
    let h = src.h as f32;
    per_pixel(src, |x, y| {
        let p = px_at(src, x, y);
        let film = (1.0 - luma(&p)).powi(2);
        let beam = (1.0 - ((y as f32 + 0.5) / h - position).abs() / 0.06).clamp(0.0, 1.0);
        pack(std::array::from_fn(|c| 255.0 * (film * tint[c] + 0.8 * beam)), p[3])
    })
}

/// Zigzag border: a `width` px frame whose inner edge runs in triangular teeth, each tooth `width` px deep at
/// its peak. The teeth are `color`, and the rest of the frame is `accent`.
pub(super) fn zigzag_border(src: &Buf, width: u32, color: Rgba, accent: Rgba) -> Result<Buf, EffectError> {
    let period = (2 * width.max(1)) as usize;
    let m = width as f32;
    let (teeth, rest) = (rgba4(color), rgba4(accent));
    frame(src, width, |x, y, depth| {
        let f = ((x + y) % period) as f32 / period as f32;
        let tooth = 1.0 - (2.0 * f - 1.0).abs();
        if (depth as f32 + 0.5) < m * tooth { teeth } else { rest }
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

/// A canvas `[left, top, right, bottom]` px larger than `w` x `h`, or `Invalid` past the pixel budget.
fn grown(w: usize, h: usize, [left, top, right, bottom]: [usize; 4]) -> Result<(usize, usize), EffectError> {
    let cw = w as u64 + left as u64 + right as u64;
    let ch = h as u64 + top as u64 + bottom as u64;
    if cw > u64::from(u32::MAX) || ch > u64::from(u32::MAX) || cw * ch > MAX_CANVAS_PIXELS {
        return Err(EffectError::Invalid(format!("a {cw} x {ch} canvas is past the pixel budget")));
    }
    Ok((cw as usize, ch as usize))
}

/// `src` on a canvas grown by `margins` (left, top, right, bottom), with `paint(x, y)` on every other pixel.
fn place(src: &Buf, margins: [usize; 4], paint: impl Fn(usize, usize) -> [u8; 4] + Sync) -> Result<Buf, EffectError> {
    let [left, top, _, _] = margins;
    let (cw, ch) = grown(src.w, src.h, margins)?;
    let mut px = vec![0u8; cw * ch * 4];
    px.par_chunks_mut(cw * 4).enumerate().for_each(|(y, row)| {
        for (x, out) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let inside = x >= left && y >= top && x - left < src.w && y - top < src.h;
            *out = if inside { px_at(src, x - left, y - top) } else { paint(x, y) };
        }
    });
    Ok(Buf { w: cw, h: ch, px })
}

/// The frame of a border: `width` px of `paint(x, y, depth)` on every side of the image, where `depth` is
/// the distance from the canvas edge. The image sits unchanged in the middle.
fn frame(src: &Buf, width: u32, paint: impl Fn(usize, usize, usize) -> [u8; 4] + Sync) -> Result<Buf, EffectError> {
    let m = width as usize;
    let (cw, ch) = grown(src.w, src.h, [m; 4])?;
    place(src, [m; 4], |x, y| paint(x, y, x.min(y).min(cw - 1 - x).min(ch - 1 - y)))
}

/// The pixel at index `i` (row-major).
fn pixel(src: &Buf, i: usize) -> [u8; 4] {
    [src.px[i * 4], src.px[i * 4 + 1], src.px[i * 4 + 2], src.px[i * 4 + 3]]
}

fn px_at(src: &Buf, x: usize, y: usize) -> [u8; 4] {
    pixel(src, y * src.w + x)
}

/// The index of `v` (floored) clamped to `0..len`. NaN becomes 0.
fn clamp_index(v: f32, len: usize) -> usize {
    v.floor().clamp(0.0, (len - 1) as f32) as usize
}

/// The pixel under `p`, clamped to the image.
fn sample(src: &Buf, p: [f32; 2]) -> [u8; 4] {
    px_at(src, clamp_index(p[0], src.w), clamp_index(p[1], src.h))
}

/// The pixel indices along one axis that a disc of radius `r` around `c` can touch, clamped to `0..len`.
fn reach(c: f32, r: f32, len: usize) -> RangeInclusive<usize> {
    clamp_index(c - r, len)..=clamp_index(c + r, len)
}

/// Blends the pixel at (`x`, `y`) toward white by `t`, keeping its alpha.
fn whiten(out: &mut Buf, x: usize, y: usize, t: f32) {
    let i = (y * out.w + x) * 4;
    for v in &mut out.px[i..i + 3] {
        *v = to_u8(f32::from(*v) + (255.0 - f32::from(*v)) * t);
    }
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

/// The colour at `t` along `stops`, which pair a position in `0..=1` (ascending) with a colour.
fn gradient(stops: &[(f32, [f32; 3])], t: f32) -> [f32; 3] {
    let t = unit(t);
    for pair in stops.windows(2) {
        let ((t0, c0), (t1, c1)) = (pair[0], pair[1]);
        if t <= t1 {
            return lerp3(c0, c1, ((t - t0) / (t1 - t0)).clamp(0.0, 1.0));
        }
    }
    stops.last().map_or([0.0; 3], |stop| stop.1)
}

/// HSL to RGB in 0..=255. `hue` is in turns and wraps; `sat` and `light` are 0..=1.
fn hsl_rgb(hue: f32, sat: f32, light: f32) -> [f32; 3] {
    let chroma = (1.0 - (2.0 * light - 1.0).abs()) * sat;
    let hp = hue.rem_euclid(1.0) * 6.0;
    let x = chroma * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = light - chroma / 2.0;
    [(r + m) * 255.0, (g + m) * 255.0, (b + m) * 255.0]
}

/// Sobel gradient strength of the luma, 0..=1 (a full-contrast step is 1), row by row in parallel.
fn sobel(src: &Buf) -> Vec<f32> {
    let (w, h) = (src.w, src.h);
    let l = luma_map(src);
    let at = |x: usize, y: usize| l[y * w + x];
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

/// Signed distance from `p` to a rounded rectangle centred at `c` with half size `half` and corner radius
/// `r`. Negative inside.
fn round_box(p: [f32; 2], c: [f32; 2], half: [f32; 2], r: f32) -> f32 {
    let qx = (p[0] - c[0]).abs() - (half[0] - r);
    let qy = (p[1] - c[1]).abs() - (half[1] - r);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

/// Uniform in `0..1` from the top 24 bits of a hash.
fn unit_of(bits: u64) -> f32 {
    (bits >> 40) as f32 / 16_777_216.0
}

/// Uniform in `0..1` for `index` under `seed`.
fn unit_hash(seed: u64, index: u64) -> f32 {
    unit_of(hash(seed, index))
}

/// A lattice point packed into one index, so neighbouring cells hash apart.
fn lattice(i: i64, j: i64) -> u64 {
    ((i as u64) << 32) ^ (j as u64 & 0xffff_ffff)
}

/// Uniform in `0..1` for lattice cell (`i`, `j`) under `seed`. `slot` picks one of several values per cell.
fn cell_unit(seed: u64, i: i64, j: i64, slot: u64) -> f32 {
    unit_of(hash(seed ^ slot.wrapping_mul(0x9E37_79B9_7F4A_7C15), lattice(i, j)))
}

/// Perlin's fade curve: 0 at 0, 1 at 1, with zero slope at both ends.
fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Gradient noise at (`x`, `y`): a seeded unit gradient at each lattice point, dotted with the offset from
/// it and blended with the fade curve. Roughly -0.7..=0.7.
fn perlin(seed: u64, x: f32, y: f32) -> f32 {
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let (i, j) = (x0 as i64, y0 as i64);
    let dot = |dx: i64, dy: i64| {
        let angle = TAU * cell_unit(seed, i + dx, j + dy, 0);
        angle.cos() * (fx - dx as f32) + angle.sin() * (fy - dy as f32)
    };
    let (u, v) = (fade(fx), fade(fy));
    let top = dot(0, 0) + (dot(1, 0) - dot(0, 0)) * u;
    let bottom = dot(0, 1) + (dot(1, 1) - dot(0, 1)) * u;
    top + (bottom - top) * v
}

/// Fractal noise of `octaves` octaves of `perlin`, each at double the frequency and half the weight, mapped
/// to 0..=1 around 0.5.
fn fbm(seed: u64, x: f32, y: f32, octaves: u64) -> f32 {
    let (mut sum, mut weight, mut amp) = (0.0f32, 0.0f32, 1.0f32);
    for o in 0..octaves {
        let f = (1u64 << o) as f32;
        sum += amp * perlin(hash(seed, o), x * f, y * f);
        weight += amp;
        amp *= 0.5;
    }
    (0.5 + 1.6 * sum / weight).clamp(0.0, 1.0)
}

/// The Voronoi cell a pixel falls in: its lattice cell, the feature point in it, the distance to that point,
/// and the distance to the second nearest point.
#[derive(Clone, Copy, Debug)]
struct VoronoiHit {
    cell: (i64, i64),
    point: [f32; 2],
    dist: f32,
    second: f32,
}

/// The nearest and second-nearest feature points to `p`, one jittered point per `size` px cell, searched in
/// the 3 x 3 cells around `p`.
fn voronoi_nearest(seed: u64, size: f32, p: [f32; 2]) -> VoronoiHit {
    let (ci, cj) = ((p[0] / size).floor() as i64, (p[1] / size).floor() as i64);
    let mut hit = VoronoiHit { cell: (ci, cj), point: [0.0; 2], dist: f32::INFINITY, second: f32::INFINITY };
    for j in cj - 1..=cj + 1 {
        for i in ci - 1..=ci + 1 {
            let q = [(i as f32 + cell_unit(seed, i, j, 4)) * size, (j as f32 + cell_unit(seed, i, j, 5)) * size];
            let d = (p[0] - q[0]).hypot(p[1] - q[1]);
            if d < hit.dist {
                hit = VoronoiHit { cell: (i, j), point: q, dist: d, second: hit.dist };
            } else if d < hit.second {
                hit.second = d;
            }
        }
    }
    hit
}
