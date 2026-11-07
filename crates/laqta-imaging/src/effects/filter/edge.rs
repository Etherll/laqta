//! Edge effects: torn and wave edges make the alpha of chosen sides transparent in a ragged band.
//! The canvas size doesn't change.
//!
//! Sides are indexed 0 top, 1 right, 2 bottom, 3 left. A side's cut depth is a function of the
//! position along it, so each side is one pass over its edge.

use std::f32::consts::{PI, TAU};

use super::{Buf, Rng};

/// Torn edge. Each enabled side cuts up to `depth` pixels in from its edge. The cut depth is drawn at
/// random knots every `range` pixels (at least 1) and interpolated between them, linearly for a jagged
/// edge or with a cosine for a `curved` one. Sides are drawn in order from one seeded stream.
pub(super) fn torn(src: &Buf, depth: u32, range: u32, sides: [bool; 4], curved: bool, seed: u64) -> Buf {
    let mut rng = Rng::new(seed);
    let spacing = range.max(1) as usize;
    let mut out = src.clone();
    for (side, on) in sides.into_iter().enumerate() {
        if !on {
            continue;
        }
        let along = along_len(src, side);
        let knots: Vec<f32> = (0..along / spacing + 2).map(|_| rng.range(0, depth) as f32).collect();
        cut_side(&mut out, side, |p| {
            let t = p as f32 / spacing as f32;
            let k = t.floor() as usize;
            let f = t - k as f32;
            let f = if curved { (1.0 - (PI * f).cos()) / 2.0 } else { f };
            let (a, b) = (knots[k], knots[k + 1]);
            (a + (b - a) * f).round() as usize
        });
    }
    out
}

/// Wave edge. Each enabled side cuts a sine wave `depth` pixels deep at its deepest, with a period of
/// `range` pixels (at least 1). The wave starts from no cut at the corner.
pub(super) fn wave(src: &Buf, depth: u32, range: u32, sides: [bool; 4]) -> Buf {
    let period = range.max(1) as f32;
    let depth = depth as f32;
    let mut out = src.clone();
    for (side, on) in sides.into_iter().enumerate() {
        if on {
            cut_side(&mut out, side, |p| (depth * 0.5 * (1.0 - (TAU * p as f32 / period).cos())).round() as usize);
        }
    }
    out
}

/// Length of a side, along its edge.
fn along_len(src: &Buf, side: usize) -> usize {
    if side.is_multiple_of(2) { src.w } else { src.h }
}

/// Makes alpha zero for the first `cut(p)` pixels in from `side` at each position `p` along it. A cut
/// deeper than the image is clipped.
fn cut_side(buf: &mut Buf, side: usize, cut: impl Fn(usize) -> usize) {
    let (w, h) = (buf.w, buf.h);
    let (along, across) = if side.is_multiple_of(2) { (w, h) } else { (h, w) };
    for p in 0..along {
        for d in 0..cut(p).min(across) {
            let (x, y) = match side {
                0 => (p, d),
                1 => (w - 1 - d, p),
                2 => (p, h - 1 - d),
                _ => (d, p),
            };
            buf.px[(y * w + x) * 4 + 3] = 0;
        }
    }
}
