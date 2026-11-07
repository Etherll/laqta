//! Raster painting on one layer's pixels: a round brush along a path, the eraser, clone and healing
//! strokes, and the fill bucket. Every stroke is clipped by the selection.
//!
//! Paint, erase and clone take the strongest dab at each pixel (a per-stroke coverage buffer), then
//! apply it once, so overlapping dabs in one stroke do not stack. Heal is applied dab by dab, since
//! each dab's colour shift depends on the pixels under it. Clone and heal read the pixels as they
//! were before the stroke, so a stroke never samples its own output.

use laqta_domain::color::Rgba;

use crate::mask::Mask;
use crate::selection::region;
use crate::surface::{PixelRect, Surface, over};

/// Brush settings. `size` is the diameter in pixels, `spacing` the step between dabs as a fraction
/// of `size`, and `hardness` the fraction of the radius that is fully opaque.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Brush {
    pub size: f32,
    pub hardness: f32,
    pub opacity: f32,
    pub spacing: f32,
    pub color: Rgba,
}

impl Default for Brush {
    fn default() -> Brush {
        Brush { size: 10.0, hardness: 0.8, opacity: 1.0, spacing: 0.25, color: Rgba::BLACK }
    }
}

/// What a stroke does to the pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrushTool {
    /// Paints `Brush::color`.
    Paint,
    /// Lowers alpha to transparent.
    Erase,
    /// Copies from `dst - offset`, in layer pixels.
    Clone { offset: (i32, i32) },
    /// Clone, then shift the source so its mean colour under the dab matches the destination's.
    Heal { offset: (i32, i32) },
}

/// The per-stroke settings every dab shares.
struct Stamp<'a> {
    radius: f32,
    hardness: f32,
    opacity: f32,
    offset: (i32, i32),
    sel: Option<&'a Mask>,
    origin: (i32, i32),
}

/// Dab coverage at distance `d` (in radii): 1 inside `hardness`, falling linearly to 0 at the edge.
fn profile(d: f32, hardness: f32) -> f32 {
    if d >= 1.0 {
        return 0.0;
    }
    let h = hardness.clamp(0.0, 0.999);
    if d <= h { 1.0 } else { (1.0 - d) / (1.0 - h) }
}

/// Positions along `path` at `spacing` intervals, starting with the first point.
fn densify(path: &[(f32, f32)], spacing: f32) -> Vec<(f32, f32)> {
    let Some(&first) = path.first() else { return Vec::new() };
    let mut out = vec![first];
    let mut to_next = spacing;
    for seg in path.windows(2) {
        let ((ax, ay), (bx, by)) = (seg[0], seg[1]);
        let len = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();
        if len <= 0.0 {
            continue;
        }
        let mut pos = 0.0;
        while len - pos >= to_next {
            pos += to_next;
            let t = pos / len;
            out.push((ax + (bx - ax) * t, ay + (by - ay) * t));
            to_next = spacing;
        }
        to_next -= len - pos;
    }
    out
}

/// Calls `f(x, y, coverage)` for every pixel the dab at `(cx, cy)` touches inside `w` x `h`.
fn for_each_dab(cx: f32, cy: f32, radius: f32, hardness: f32, w: u32, h: u32, mut f: impl FnMut(usize, usize, f32)) {
    let x0 = (cx - radius).floor() as i64;
    let x1 = (cx + radius).ceil() as i64;
    let y0 = (cy - radius).floor() as i64;
    let y1 = (cy + radius).ceil() as i64;
    for y in y0.max(0)..=y1.min(i64::from(h) - 1) {
        for x in x0.max(0)..=x1.min(i64::from(w) - 1) {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let p = profile((dx * dx + dy * dy).sqrt() / radius, hardness);
            if p > 0.0 {
                f(x as usize, y as usize, p);
            }
        }
    }
}

/// The selection's coverage at a layer-local pixel. `origin` is the layer's document offset.
fn selected(sel: Option<&Mask>, origin: (i32, i32), x: usize, y: usize) -> f32 {
    sel.map_or(1.0, |m| m.coverage(x as i32 + origin.0, y as i32 + origin.1))
}

/// The bounding box of the pixels a stroke wrote, grown as pixels are added.
#[derive(Default)]
struct Dirty {
    bounds: Option<(usize, usize, usize, usize)>,
}

impl Dirty {
    fn add(&mut self, x: usize, y: usize) {
        self.bounds = Some(match self.bounds {
            None => (x, y, x, y),
            Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
        });
    }

    fn rect(&self) -> Option<PixelRect> {
        self.bounds.map(|(x0, y0, x1, y1)| PixelRect { x: x0 as u32, y: y0 as u32, w: (x1 - x0 + 1) as u32, h: (y1 - y0 + 1) as u32 })
    }
}

/// Runs a stroke over `dst`. Returns the rectangle that changed, or `None` if nothing did.
pub(crate) fn stroke(
    dst: &mut Surface,
    path: &[(f32, f32)],
    brush: &Brush,
    tool: BrushTool,
    sel: Option<&Mask>,
    origin: (i32, i32),
) -> Option<PixelRect> {
    let dabs = densify(path, (brush.size * brush.spacing).max(1.0));
    if dabs.is_empty() {
        return None;
    }
    let stamp = Stamp {
        radius: (brush.size / 2.0).max(0.5),
        hardness: brush.hardness,
        opacity: brush.opacity.clamp(0.0, 1.0),
        offset: match tool {
            BrushTool::Clone { offset } | BrushTool::Heal { offset } => offset,
            BrushTool::Paint | BrushTool::Erase => (0, 0),
        },
        sel,
        origin,
    };
    let mut dirty = Dirty::default();
    match tool {
        BrushTool::Heal { .. } => {
            // Each dab's colour shift depends on the pixels under it, so heal runs dab by dab.
            let source = dst.clone();
            for &centre in &dabs {
                heal_dab(dst, &source, centre, &stamp, &mut dirty);
            }
        }
        BrushTool::Paint | BrushTool::Erase | BrushTool::Clone { .. } => {
            let source = matches!(tool, BrushTool::Clone { .. }).then(|| dst.clone());
            let (w, h) = (dst.w, dst.h);
            let mut coverage = vec![0.0_f32; w as usize * h as usize];
            for &(cx, cy) in &dabs {
                for_each_dab(cx, cy, stamp.radius, stamp.hardness, w, h, |x, y, p| {
                    let i = y * w as usize + x;
                    coverage[i] = coverage[i].max(p);
                    dirty.add(x, y);
                });
            }
            let color = [f32::from(brush.color.r), f32::from(brush.color.g), f32::from(brush.color.b)];
            let color_alpha = f32::from(brush.color.a) / 255.0;
            for (i, &cov) in coverage.iter().enumerate() {
                if cov <= 0.0 {
                    continue;
                }
                let (x, y) = (i % w as usize, i / w as usize);
                let a = cov * stamp.opacity * selected(sel, origin, x, y);
                if a <= 0.0 {
                    continue;
                }
                let p = i * 4;
                match tool {
                    BrushTool::Erase => {
                        let kept = 1.0 - a;
                        dst.px[p + 3] = (f32::from(dst.px[p + 3]) * kept).round() as u8;
                    }
                    BrushTool::Clone { offset } => {
                        let from = (x as i64 - i64::from(offset.0), y as i64 - i64::from(offset.1));
                        let sampled = source.as_ref().and_then(|s| s.pixel(from.0, from.1));
                        if let Some(sp) = sampled {
                            let rgb = [f32::from(sp[0]), f32::from(sp[1]), f32::from(sp[2])];
                            over(&mut dst.px[p..p + 4], rgb, a * f32::from(sp[3]) / 255.0);
                        }
                    }
                    _ => over(&mut dst.px[p..p + 4], color, a * color_alpha),
                }
            }
        }
    }
    dirty.rect()
}

/// One heal dab: shift the source colours by the difference of the two means under the dab, then
/// composite them over the destination.
fn heal_dab(dst: &mut Surface, source: &Surface, centre: (f32, f32), stamp: &Stamp<'_>, dirty: &mut Dirty) {
    let (w, h) = (dst.w, dst.h);
    let (ox, oy) = (i64::from(stamp.offset.0), i64::from(stamp.offset.1));
    let (cx, cy) = centre;
    // Mean colour difference between the destination under the dab and the source it samples.
    let mut sum_d = [0.0_f32; 3];
    let mut sum_s = [0.0_f32; 3];
    let mut n = 0usize;
    for_each_dab(cx, cy, stamp.radius, stamp.hardness, w, h, |x, y, _| {
        let Some(sp) = source.pixel(x as i64 - ox, y as i64 - oy) else { return };
        let dp = dst.pixel(x as i64, y as i64).unwrap_or([0; 4]);
        for ((d, s), (dv, sv)) in sum_d.iter_mut().zip(sum_s.iter_mut()).zip(dp.iter().zip(sp.iter())) {
            *d += f32::from(*dv);
            *s += f32::from(*sv);
        }
        n += 1;
    });
    if n == 0 {
        return;
    }
    let delta: [f32; 3] = std::array::from_fn(|c| (sum_d[c] - sum_s[c]) / n as f32);
    for_each_dab(cx, cy, stamp.radius, stamp.hardness, w, h, |x, y, p| {
        let Some(sp) = source.pixel(x as i64 - ox, y as i64 - oy) else { return };
        let a = p * stamp.opacity * selected(stamp.sel, stamp.origin, x, y) * f32::from(sp[3]) / 255.0;
        if a <= 0.0 {
            return;
        }
        let rgb: [f32; 3] = std::array::from_fn(|c| (f32::from(sp[c]) + delta[c]).clamp(0.0, 255.0));
        let i = (y * w as usize + x) * 4;
        over(&mut dst.px[i..i + 4], rgb, a);
        dirty.add(x, y);
    });
}

/// Fills the region matching the pixel at `(x, y)` within `tol`. Returns the changed rectangle.
pub(crate) fn bucket(
    dst: &mut Surface,
    (x, y): (i32, i32),
    tol: u8,
    color: Rgba,
    contiguous: bool,
    sel: Option<&Mask>,
    origin: (i32, i32),
) -> Option<PixelRect> {
    let matched = region(dst, x, y, tol, contiguous)?;
    let w = dst.w as usize;
    let rgb = [f32::from(color.r), f32::from(color.g), f32::from(color.b)];
    let color_alpha = f32::from(color.a) / 255.0;
    let mut dirty = Dirty::default();
    for (i, &m) in matched.iter().enumerate() {
        if m == 0 {
            continue;
        }
        let (px, py) = (i % w, i / w);
        let k = selected(sel, origin, px, py);
        if k <= 0.0 {
            continue;
        }
        over(&mut dst.px[i * 4..i * 4 + 4], rgb, k * color_alpha);
        dirty.add(px, py);
    }
    dirty.rect()
}
