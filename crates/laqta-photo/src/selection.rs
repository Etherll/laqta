//! Selections are document-sized masks. Shapes are rasterised with anti-aliasing; the magic wand is
//! a flood fill of the rendered composite.

use tiny_skia::{FillRule, PathBuilder, Transform};

use crate::mask::Mask;
use crate::surface::{Surface, within};

/// How a new selection combines with the current one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectOp {
    Replace,
    Add,
    Subtract,
    Intersect,
}

/// A shape to select, in document coordinates.
#[derive(Clone, Debug, PartialEq)]
pub enum SelectShape {
    /// The whole canvas.
    All,
    Rect {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    },
    Ellipse {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    },
    /// A lasso, filled even-odd.
    Polygon(Vec<(f32, f32)>),
    /// Pixels of the rendered composite within `tolerance` of the seed `(x, y)`. Contiguous floods
    /// from the seed; otherwise every matching pixel is taken.
    MagicWand {
        x: i32,
        y: i32,
        tolerance: u8,
        contiguous: bool,
    },
}

/// The mask of a geometric shape, or `None` for a wand (which needs pixels) or degenerate geometry.
pub(crate) fn geometry_mask(shape: &SelectShape, w: u32, h: u32) -> Option<Mask> {
    let mut pb = PathBuilder::new();
    let rule = match *shape {
        SelectShape::All => return Some(Mask::filled(w, h, 255)),
        SelectShape::Rect { x, y, width, height } => {
            pb.push_rect(tiny_skia::Rect::from_ltrb(x, y, x + width, y + height)?);
            FillRule::Winding
        }
        SelectShape::Ellipse { x, y, width, height } => {
            let path = PathBuilder::from_oval(tiny_skia::Rect::from_ltrb(x, y, x + width, y + height)?)?;
            return rasterise(&path, FillRule::Winding, w, h);
        }
        SelectShape::Polygon(ref points) => {
            let (first, rest) = points.split_first()?;
            pb.move_to(first.0, first.1);
            for p in rest {
                pb.line_to(p.0, p.1);
            }
            pb.close();
            FillRule::EvenOdd
        }
        SelectShape::MagicWand { .. } => return None,
    };
    rasterise(&pb.finish()?, rule, w, h)
}

fn rasterise(path: &tiny_skia::Path, rule: FillRule, w: u32, h: u32) -> Option<Mask> {
    let mut m = tiny_skia::Mask::new(w, h)?;
    m.fill_path(path, rule, true, Transform::identity());
    Mask::from_vec(w, h, m.data().to_vec()).ok()
}

/// The new selection after combining `shape` with `current` (`None` is an empty selection here).
pub(crate) fn combine(current: Option<&Mask>, shape: &Mask, op: SelectOp) -> Mask {
    if op == SelectOp::Replace {
        return shape.clone();
    }
    let cur: Vec<u8> = match current {
        Some(m) => m.data().to_vec(),
        None => vec![0; shape.data().len()],
    };
    let data = cur
        .iter()
        .zip(shape.data())
        .map(|(&c, &s)| match op {
            SelectOp::Replace => s,
            SelectOp::Add => c.max(s),
            SelectOp::Subtract => ((u32::from(c) * u32::from(255 - s) + 127) / 255) as u8,
            SelectOp::Intersect => c.min(s),
        })
        .collect();
    Mask::from_vec(shape.width(), shape.height(), data).unwrap_or_else(|_| shape.clone())
}

/// Select all when there is no selection, otherwise the complement.
pub(crate) fn invert(current: Option<&Mask>, w: u32, h: u32) -> Mask {
    match current {
        Some(m) => m.inverted(),
        None => Mask::filled(w, h, 255),
    }
}

/// The pixels matching the seed colour at `(x, y)` (255 in the result), or `None` when the seed is
/// outside the surface. A contiguous flood uses an explicit stack, so large regions cannot recurse.
pub(crate) fn region(s: &Surface, x: i32, y: i32, tol: u8, contiguous: bool) -> Option<Vec<u8>> {
    let seed = s.pixel(i64::from(x), i64::from(y))?;
    let (w, h) = (s.w as usize, s.h as usize);
    let mut out = vec![0u8; w * h];
    let matches = |i: usize| within(pixel_at(s, i), seed, tol);
    if contiguous {
        let start = y as usize * w + x as usize;
        out[start] = 255;
        let mut stack = vec![start];
        while let Some(i) = stack.pop() {
            let (px, py) = (i % w, i / w);
            let mut neighbours = [None; 4];
            if px > 0 {
                neighbours[0] = Some(i - 1);
            }
            if px + 1 < w {
                neighbours[1] = Some(i + 1);
            }
            if py > 0 {
                neighbours[2] = Some(i - w);
            }
            if py + 1 < h {
                neighbours[3] = Some(i + w);
            }
            for n in neighbours.into_iter().flatten() {
                if out[n] == 0 && matches(n) {
                    out[n] = 255;
                    stack.push(n);
                }
            }
        }
    } else {
        for (i, v) in out.iter_mut().enumerate() {
            if matches(i) {
                *v = 255;
            }
        }
    }
    Some(out)
}

fn pixel_at(s: &Surface, i: usize) -> [u8; 4] {
    let p = i * 4;
    [s.px[p], s.px[p + 1], s.px[p + 2], s.px[p + 3]]
}

/// Grows (`radius > 0`) or shrinks a mask. Used by the selection commands.
pub(crate) fn grow(m: &Mask, radius: u32) -> Mask {
    m.dilate(radius)
}

pub(crate) fn shrink(m: &Mask, radius: u32) -> Mask {
    m.erode(radius)
}
