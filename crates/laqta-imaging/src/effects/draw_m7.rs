//! Kernels for the 6 M7 drawings (X1 batch, M7 D5): paper confetti, a line, a shape, a pointer, a
//! gradient overlay and snowflakes. Each draws onto a copy of the image. Edges are antialiased over
//! about one pixel, from distances to the outline, the way `rounded_coverage` does it.

use laqta_domain::color::Rgba;
use laqta_domain::effects::draw::ShapeKind;

use super::draw::{Buf, SplitMix64, lerp};

/// Confetti colours, as RGBA8.
const PALETTE: [[u8; 4]; 6] =
    [[230, 57, 70, 255], [244, 162, 97, 255], [233, 196, 106, 255], [42, 157, 143, 255], [69, 123, 157, 255], [157, 78, 221, 255]];

/// The pointer outline in units of its height: the tip at the origin, the shaft down the left side,
/// and the tail at the bottom.
const POINTER: [(f64, f64); 7] = [(0.0, 0.0), (0.0, 0.8), (0.22, 0.62), (0.36, 0.95), (0.5, 0.89), (0.36, 0.56), (0.62, 0.56)];

/// Distance from `p` to the segment from `a` to `b`.
fn segment_distance(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (vx, vy) = (b.0 - a.0, b.1 - a.1);
    let len2 = vx * vx + vy * vy;
    let t = if len2 > 0.0 { (((p.0 - a.0) * vx + (p.1 - a.1) * vy) / len2).clamp(0.0, 1.0) } else { 0.0 };
    (p.0 - a.0 - t * vx).hypot(p.1 - a.1 - t * vy)
}

/// Distance from `p` to the nearest edge of a closed polygon.
fn edge_distance(p: (f64, f64), poly: &[(f64, f64)]) -> f64 {
    let n = poly.len();
    (0..n).map(|i| segment_distance(p, poly[i], poly[(i + 1) % n])).fold(f64::INFINITY, f64::min)
}

/// Even-odd test for a point inside a closed polygon.
fn point_in_polygon(p: (f64, f64), poly: &[(f64, f64)]) -> bool {
    let n = poly.len();
    let mut inside = false;
    for (i, &(xi, yi)) in poly.iter().enumerate() {
        let (xj, yj) = poly[(i + n - 1) % n];
        if (yi > p.1) != (yj > p.1) && p.0 < (xj - xi) * (p.1 - yi) / (yj - yi) + xi {
            inside = !inside;
        }
    }
    inside
}

/// The inclusive pixel box covering `[x0, x1] x [y0, y1]`, clipped to a `w` x `h` buffer. `None`
/// when nothing is left.
fn clip_box(x0: f64, y0: f64, x1: f64, y1: f64, w: usize, h: usize) -> Option<(usize, usize, usize, usize)> {
    if w == 0 || h == 0 {
        return None;
    }
    let (lo_x, lo_y) = (x0.floor().max(0.0), y0.floor().max(0.0));
    let (hi_x, hi_y) = (x1.ceil().min(w as f64 - 1.0), y1.ceil().min(h as f64 - 1.0));
    if hi_x < lo_x || hi_y < lo_y {
        return None;
    }
    Some((lo_x as usize, lo_y as usize, hi_x as usize, hi_y as usize))
}

/// `count` paper pieces, each `sizes.0` to `sizes.1` pixels long, at seeded positions and angles, in
/// colours from the palette. One seed gives one layout.
pub(super) fn confetti(base: &Buf, count: u32, sizes: (u32, u32), seed: u64) -> Buf {
    let mut canvas = base.clone();
    let (w, h) = (base.w, base.h);
    let lo = u64::from(sizes.0.min(sizes.1).max(1));
    let hi = u64::from(sizes.0.max(sizes.1).max(1));
    let mut rng = SplitMix64(seed);
    for _ in 0..count {
        let long = (lo + rng.below(hi - lo + 1)) as f64;
        let (pw, ph) = (long, (long / 2.0).max(1.0));
        let angle = (rng.below(360) as f64).to_radians();
        let cx = rng.below(w as u64) as f64 + 0.5;
        let cy = rng.below(h as u64) as f64 + 0.5;
        let colour = PALETTE[rng.below(PALETTE.len() as u64) as usize];
        let reach = (pw.hypot(ph) / 2.0).ceil() + 1.0;
        let Some((x0, y0, x1, y1)) = clip_box(cx - reach, cy - reach, cx + reach, cy + reach, w, h) else {
            continue;
        };
        let (s, c) = angle.sin_cos();
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (dx, dy) = (x as f64 + 0.5 - cx, y as f64 + 0.5 - cy);
                let (u, v) = (dx * c + dy * s, -dx * s + dy * c);
                if u.abs() <= pw / 2.0 && v.abs() <= ph / 2.0 {
                    canvas.blend_px(x, y, colour, 1.0);
                }
            }
        }
    }
    canvas
}

/// A line from `from` to `to`, in pixel coordinates, `width` pixels thick.
pub(super) fn line(base: &Buf, from: (i32, i32), to: (i32, i32), width: u32, colour: [u8; 4]) -> Buf {
    let mut canvas = base.clone();
    let a = (f64::from(from.0), f64::from(from.1));
    let b = (f64::from(to.0), f64::from(to.1));
    let half = f64::from(width.max(1)) / 2.0;
    let reach = half + 1.0;
    let bounds = clip_box(a.0.min(b.0) - reach, a.1.min(b.1) - reach, a.0.max(b.0) + reach, a.1.max(b.1) + reach, base.w, base.h);
    let Some((x0, y0, x1, y1)) = bounds else {
        return canvas;
    };
    for y in y0..=y1 {
        for x in x0..=x1 {
            let p = (x as f64 + 0.5, y as f64 + 0.5);
            let cov = (half + 0.5 - segment_distance(p, a, b)).clamp(0.0, 1.0);
            if cov > 0.0 {
                canvas.blend_px(x, y, colour, cov as f32);
            }
        }
    }
    canvas
}

/// A rectangle, ellipse or triangle in the box (`x`, `y`, `width`, `height`). The fill covers the
/// inside, and the stroke is `stroke_width` pixels, centred on the outline.
pub(super) fn shape(
    base: &Buf,
    kind: ShapeKind,
    rect: (i32, i32, u32, u32),
    fill: Option<[u8; 4]>,
    stroke: [u8; 4],
    stroke_width: u32,
) -> Buf {
    let (x, y, rw, rh) = rect;
    let mut canvas = base.clone();
    if rw == 0 || rh == 0 {
        return canvas;
    }
    let (x0, y0) = (f64::from(x), f64::from(y));
    let (x1, y1) = (x0 + f64::from(rw), y0 + f64::from(rh));
    let half = f64::from(stroke_width) / 2.0;
    let reach = half + 1.0;
    let Some((bx0, by0, bx1, by1)) = clip_box(x0 - reach, y0 - reach, x1 + reach, y1 + reach, base.w, base.h) else {
        return canvas;
    };
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let (rx, ry) = ((x1 - x0) / 2.0, (y1 - y0) / 2.0);
    let polygon = match kind {
        ShapeKind::Rectangle => vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)],
        ShapeKind::Triangle => vec![(cx, y0), (x1, y1), (x0, y1)],
        ShapeKind::Ellipse => Vec::new(),
    };
    let norm = |scale_x: f64, scale_y: f64, p: (f64, f64)| ((p.0 - cx) / scale_x).powi(2) + ((p.1 - cy) / scale_y).powi(2);
    for py in by0..=by1 {
        for px in bx0..=bx1 {
            let p = (px as f64 + 0.5, py as f64 + 0.5);
            let (is_inside, on_stroke) = match kind {
                ShapeKind::Ellipse => {
                    let inside = norm(rx, ry, p) <= 1.0;
                    let outer = norm(rx + half, ry + half, p) <= 1.0;
                    let inner = rx > half && ry > half && norm(rx - half, ry - half, p) < 1.0;
                    (inside, outer && !inner)
                }
                _ => (point_in_polygon(p, &polygon), edge_distance(p, &polygon) <= half),
            };
            if let Some(f) = fill
                && is_inside
            {
                canvas.blend_px(px, py, f, 1.0);
            }
            if half > 0.0 && on_stroke {
                canvas.blend_px(px, py, stroke, 1.0);
            }
        }
    }
    canvas
}

/// A mouse pointer with its tip at `at`, `size` pixels tall. It is white with a black outline, and
/// an optional translucent shadow falls down and to the right.
pub(super) fn cursor(base: &Buf, at: (i32, i32), size: u32, shadow: bool) -> Buf {
    let mut canvas = base.clone();
    let s = f64::from(size.max(1));
    let tip = (f64::from(at.0), f64::from(at.1));
    let poly: Vec<(f64, f64)> = POINTER.iter().map(|&(u, v)| (tip.0 + u * s, tip.1 + v * s)).collect();
    let outline = (s / 12.0).max(1.0);
    let drop = (s / 12.0).max(1.0);
    let shifted: Vec<(f64, f64)> = poly.iter().map(|&(px, py)| (px + drop, py + drop)).collect();
    let min_x = poly.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    let min_y = poly.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let max_x = poly.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
    let max_y = poly.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    let reach = outline + drop + 1.0;
    let Some((x0, y0, x1, y1)) = clip_box(min_x - reach, min_y - reach, max_x + reach, max_y + reach, base.w, base.h) else {
        return canvas;
    };
    for py in y0..=y1 {
        for px in x0..=x1 {
            let p = (px as f64 + 0.5, py as f64 + 0.5);
            if shadow && point_in_polygon(p, &shifted) {
                canvas.blend_px(px, py, [0, 0, 0, 255], 0.4);
            }
            if edge_distance(p, &poly) <= outline / 2.0 {
                canvas.blend_px(px, py, [0, 0, 0, 255], 1.0);
            } else if point_in_polygon(p, &poly) {
                canvas.blend_px(px, py, [255, 255, 255, 255], 1.0);
            }
        }
    }
    canvas
}

/// A linear gradient from `start` to `end`, laid over the image at `angle` degrees (0 runs left to
/// right). `opacity` scales the result.
pub(super) fn gradient_overlay(base: &Buf, start: Rgba, end: Rgba, angle: f32, opacity: f32) -> Buf {
    let mut canvas = base.clone();
    let (w, h) = (base.w, base.h);
    let (dy, dx) = f64::from(angle).to_radians().sin_cos();
    let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
    // Half the image's extent along the gradient direction maps to 0 and 1.
    let half = (w as f64 * dx.abs() + h as f64 * dy.abs()) / 2.0;
    let o = opacity.clamp(0.0, 1.0);
    for y in 0..h {
        for x in 0..w {
            let along = (x as f64 + 0.5 - cx) * dx + (y as f64 + 0.5 - cy) * dy;
            let t = if half > 0.0 { ((along / half + 1.0) / 2.0).clamp(0.0, 1.0) } else { 0.5 };
            canvas.blend_px(x, y, lerp(start, end, t as f32), o);
        }
    }
    canvas
}

/// `count` soft white flakes, `sizes.0` to `sizes.1` pixels across, at seeded positions.
pub(super) fn snowflakes(base: &Buf, count: u32, sizes: (u32, u32), seed: u64) -> Buf {
    let mut canvas = base.clone();
    let (w, h) = (base.w, base.h);
    let lo = u64::from(sizes.0.min(sizes.1).max(1));
    let hi = u64::from(sizes.0.max(sizes.1).max(1));
    let mut rng = SplitMix64(seed);
    for _ in 0..count {
        let radius = (lo + rng.below(hi - lo + 1)) as f64 / 2.0;
        let cx = rng.below(w as u64) as f64 + 0.5;
        let cy = rng.below(h as u64) as f64 + 0.5;
        let Some((x0, y0, x1, y1)) = clip_box(cx - radius - 1.0, cy - radius - 1.0, cx + radius + 1.0, cy + radius + 1.0, w, h) else {
            continue;
        };
        for y in y0..=y1 {
            for x in x0..=x1 {
                let d = (x as f64 + 0.5 - cx).hypot(y as f64 + 0.5 - cy);
                let cov = (radius + 0.5 - d).clamp(0.0, 1.0);
                if cov > 0.0 {
                    canvas.blend_px(x, y, [255, 255, 255, 255], (0.9 * cov) as f32);
                }
            }
        }
    }
    canvas
}
