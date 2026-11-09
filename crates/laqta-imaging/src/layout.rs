//! Regions inside a window, from pixels only (docs/plan/LAYOUT.md): the panels a UI is split
//! into (sidebars, headers, columns) and the boxes inside them (inputs, cards, buttons), so the
//! region overlay can snap to them where no accessibility tree is available.
//!
//! Three passes over one edge map:
//! - **Panels**: a recursive XY-cut on rows and columns that are almost entirely edge pixels
//!   (a divider line or a change of background across the panel).
//! - **Boxes**: pairs of long horizontal edge runs with matching ends, closed by vertical edges on
//!   both sides (inputs, cards, buttons, chips; rounded corners allowed).
//! - **Content blocks**: inside each panel and box, content split on blank gaps, with the gap size
//!   taken from the content itself, so paragraphs split before lines and table rows before cells.
//!
//! Everything is prefix sums and linear scans over one luminance pass: a 2000×1100 window takes
//! about 10 ms on one core.

use laqta_domain::{Bitmap, PhysRect, PixelFormat};

/// Luminance difference that counts as an edge. Dark UIs separate panels by about 14 levels.
const EDGE: u8 = 10;
/// Share of a row (column) that must be edge pixels for it to cut a panel.
const COVER: f32 = 0.85;
/// The smallest panel side, in pixels.
const MIN_SIZE: i32 = 24;
/// How many times a panel is split again.
const MAX_DEPTH: u32 = 4;
/// The shortest horizontal edge run that can be a box's top or bottom.
const MIN_RUN: usize = 40;
/// How far a box's top and bottom ends may differ (rounded corners).
const END_SLACK: i32 = 12;
/// Share of a box side that must be vertical edge pixels.
const SIDE_COVER: f32 = 0.6;
/// The smallest box.
const MIN_BOX: (i32, i32) = (24, 14);
/// Regions that overlap this much are one region.
const SAME_IOU: f32 = 0.92;
/// How many regions one window yields at most.
const MAX_REGIONS: usize = 300;

/// The window's edge maps and prefix sums.
struct Edges {
    w: usize,
    h: usize,
    /// `row[y * (w + 1) + x]`: horizontal-edge pixels in row `y` before column `x`.
    row: Vec<u16>,
    /// `col[x * (h + 1) + y]`: vertical-edge pixels in column `x` above row `y`.
    col: Vec<u16>,
    /// Horizontal-edge flags, row-major.
    hflag: Vec<bool>,
    /// `ink_row[y * (w + 1) + x]`: edge pixels of either kind in row `y` before column `x`.
    ink_row: Vec<u16>,
    /// `ink_col[x * (h + 1) + y]`: edge pixels of either kind in column `x` above row `y`.
    ink_col: Vec<u16>,
}

/// An edge between `a` and its neighbour `b` one pixel back, or `c` two back: anti-aliased
/// borders spread one step over two pixels (0 → 8 → 14), so the two-pixel step counts too.
fn is_edge(a: u8, b: u8, c: Option<u8>) -> bool {
    a.abs_diff(b) >= EDGE || c.is_some_and(|c| a.abs_diff(c) >= EDGE + 2 && a.abs_diff(b) >= 3)
}

impl Edges {
    fn new(image: &Bitmap, win: PhysRect) -> Edges {
        let (w, h) = (win.width as usize, win.height as usize);
        let stride = image.width() as usize * 4;
        let px = image.pixels();
        let (ri, bi) = match image.format() {
            PixelFormat::Rgba8 => (0, 2),
            PixelFormat::Bgra8 => (2, 0),
        };
        let mut luma = vec![0u8; w * h];
        for y in 0..h {
            let row = &px[(win.y as usize + y) * stride + win.x as usize * 4..][..w * 4];
            for (x, p) in row.as_chunks::<4>().0.iter().enumerate() {
                let l = (u32::from(p[ri]) * 54 + u32::from(p[1]) * 183 + u32::from(p[bi]) * 19) >> 8;
                luma[y * w + x] = l as u8;
            }
        }
        let mut hflag = vec![false; w * h];
        let mut vflag = vec![false; w * h];
        for y in 0..h {
            for x in 0..w {
                let l = luma[y * w + x];
                if y > 0 && is_edge(l, luma[(y - 1) * w + x], (y > 1).then(|| luma[(y - 2) * w + x])) {
                    hflag[y * w + x] = true;
                }
                if x > 0 && is_edge(l, luma[y * w + x - 1], (x > 1).then(|| luma[y * w + x - 2])) {
                    vflag[y * w + x] = true;
                }
            }
        }
        let mut row = vec![0u16; h * (w + 1)];
        for y in 0..h {
            for x in 0..w {
                row[y * (w + 1) + x + 1] = row[y * (w + 1) + x] + u16::from(hflag[y * w + x]);
            }
        }
        let mut col = vec![0u16; w * (h + 1)];
        for x in 0..w {
            for y in 0..h {
                col[x * (h + 1) + y + 1] = col[x * (h + 1) + y] + u16::from(vflag[y * w + x]);
            }
        }
        let mut ink_row = vec![0u16; h * (w + 1)];
        for y in 0..h {
            for x in 0..w {
                let ink = hflag[y * w + x] || vflag[y * w + x];
                ink_row[y * (w + 1) + x + 1] = ink_row[y * (w + 1) + x] + u16::from(ink);
            }
        }
        let mut ink_col = vec![0u16; w * (h + 1)];
        for x in 0..w {
            for y in 0..h {
                let ink = hflag[y * w + x] || vflag[y * w + x];
                ink_col[x * (h + 1) + y + 1] = ink_col[x * (h + 1) + y] + u16::from(ink);
            }
        }
        Edges { w, h, row, col, hflag, ink_row, ink_col }
    }

    /// Whether row `y` has any edge pixel between columns `x0..x1`.
    fn row_inked(&self, y: usize, x0: usize, x1: usize) -> bool {
        let base = y * (self.w + 1);
        self.ink_row[base + x1] > self.ink_row[base + x0]
    }

    /// Whether column `x` has any edge pixel between rows `y0..y1`.
    fn col_inked(&self, x: usize, y0: usize, y1: usize) -> bool {
        let base = x * (self.h + 1);
        self.ink_col[base + y1] > self.ink_col[base + y0]
    }

    /// Share of row `y` between columns `x0..x1` that are horizontal edges.
    fn row_cover(&self, y: usize, x0: usize, x1: usize) -> f32 {
        let base = y * (self.w + 1);
        f32::from(self.row[base + x1] - self.row[base + x0]) / (x1 - x0).max(1) as f32
    }

    /// Share of column `x` between rows `y0..y1` that are vertical edges.
    fn col_cover(&self, x: usize, y0: usize, y1: usize) -> f32 {
        let base = x * (self.h + 1);
        f32::from(self.col[base + y1] - self.col[base + y0]) / (y1 - y0).max(1) as f32
    }
}

/// A rect in window coordinates (half-open).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct R {
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
}

impl R {
    fn w(self) -> i32 {
        self.x1 - self.x0
    }
    fn h(self) -> i32 {
        self.y1 - self.y0
    }
    fn area(self) -> i64 {
        i64::from(self.w().max(0)) * i64::from(self.h().max(0))
    }
    fn iou(self, o: R) -> f32 {
        let i = R { x0: self.x0.max(o.x0), y0: self.y0.max(o.y0), x1: self.x1.min(o.x1), y1: self.y1.min(o.y1) };
        if i.w() <= 0 || i.h() <= 0 {
            return 0.0;
        }
        let inter = i.area() as f32;
        inter / (self.area() + o.area()) as f32 - inter
    }
    fn near(self, o: R, slack: i32) -> bool {
        (self.x0 - o.x0).abs() <= slack
            && (self.y0 - o.y0).abs() <= slack
            && (self.x1 - o.x1).abs() <= slack
            && (self.y1 - o.y1).abs() <= slack
    }
}

/// Cut positions along one axis: lines that cover the rect, with nearby ones merged.
fn cuts(e: &Edges, r: R, horizontal: bool) -> Vec<i32> {
    let mut out: Vec<i32> = Vec::new();
    let (from, to) = if horizontal { (r.y0 + 1, r.y1 - 1) } else { (r.x0 + 1, r.x1 - 1) };
    for p in from..to {
        let cover = if horizontal {
            e.row_cover(p as usize, r.x0 as usize, r.x1 as usize)
        } else {
            e.col_cover(p as usize, r.y0 as usize, r.y1 as usize)
        };
        if cover >= COVER {
            // A divider drawn 2 px thick, or anti-aliased, is one cut.
            if out.last().is_some_and(|&last| p - last <= 3) {
                continue;
            }
            out.push(p);
        }
    }
    out
}

/// The strips between `cuts` inside `lo..hi`; strips thinner than `MIN_SIZE` join their neighbour.
fn strips(lo: i32, hi: i32, cuts: &[i32]) -> Vec<(i32, i32)> {
    let mut out: Vec<(i32, i32)> = Vec::new();
    let mut start = lo;
    for &c in cuts.iter().chain(std::iter::once(&hi)) {
        if c - start >= MIN_SIZE {
            out.push((start, c));
            start = c;
        } else if c == hi {
            match out.last_mut() {
                Some(last) => last.1 = hi,
                None => out.push((start, hi)),
            }
        }
    }
    out
}

/// Recursive XY-cut: splits `r` along the axis that gives more strips, then each strip again.
fn xy_cut(e: &Edges, r: R, depth: u32, out: &mut Vec<R>) {
    if depth >= MAX_DEPTH || r.w() < 2 * MIN_SIZE && r.h() < 2 * MIN_SIZE {
        return;
    }
    let rows = strips(r.y0, r.y1, &cuts(e, r, true));
    let cols = strips(r.x0, r.x1, &cuts(e, r, false));
    let parts: Vec<R> = if cols.len() >= 2 && cols.len() >= rows.len() {
        cols.iter().map(|&(a, b)| R { x0: a, x1: b, ..r }).collect()
    } else if rows.len() >= 2 {
        rows.iter().map(|&(a, b)| R { y0: a, y1: b, ..r }).collect()
    } else {
        return;
    };
    for p in parts {
        out.push(p);
        xy_cut(e, p, depth + 1, out);
    }
}

/// Long horizontal edge runs: `(y, x0, x1)`, gaps of up to 2 px bridged.
fn runs(e: &Edges) -> Vec<(i32, i32, i32)> {
    let mut out = Vec::new();
    for y in 1..e.h {
        let mut x = 0;
        while x < e.w {
            if !e.hflag[y * e.w + x] {
                x += 1;
                continue;
            }
            let start = x;
            let mut end = x;
            let mut gap = 0;
            while x < e.w {
                if e.hflag[y * e.w + x] {
                    end = x;
                    gap = 0;
                } else {
                    gap += 1;
                    if gap > 2 {
                        break;
                    }
                }
                x += 1;
            }
            if end + 1 - start >= MIN_RUN {
                out.push((y as i32, start as i32, end as i32 + 1));
            }
        }
    }
    out
}

/// Boxes: a top run and the nearest bottom run with matching ends, closed by vertical edges.
fn boxes(e: &Edges) -> Vec<R> {
    let runs = runs(e);
    let mut out = Vec::new();
    for (i, &(ty, tx0, tx1)) in runs.iter().enumerate() {
        // Skip the second row of a 2 px border: the run above it already starts this box.
        if i > 0
            && runs[..i].iter().rev().take_while(|r| ty - r.0 <= 2).any(|r| (r.1 - tx0).abs() <= 2 && (r.2 - tx1).abs() <= 2 && r.0 < ty)
        {
            continue;
        }
        let bottom = runs[i + 1..]
            .iter()
            .find(|&&(by, bx0, bx1)| by - ty >= MIN_BOX.1 && (bx0 - tx0).abs() <= END_SLACK && (bx1 - tx1).abs() <= END_SLACK);
        let Some(&(by, bx0, bx1)) = bottom else { continue };
        let (x0, x1) = (tx0.min(bx0), tx1.max(bx1));
        // Rounded corners: test the sides away from the corners.
        let inset = ((by - ty) / 4).min(10);
        let (y0, y1) = ((ty + inset) as usize, (by - inset).max(ty + inset + 1) as usize);
        // The side sits up to a corner radius outside the run's end (the run starts where the
        // corner's curve ends); the outermost column that is mostly edge is the side.
        let side = |range: std::ops::RangeInclusive<i32>, outward_first: bool| {
            let cols: Vec<i32> = range.filter(|&c| c >= 0 && (c as usize) < e.w).collect();
            let ok = |c: &&i32| e.col_cover(**c as usize, y0, y1) >= SIDE_COVER;
            if outward_first { cols.iter().find(ok).copied() } else { cols.iter().rev().find(ok).copied() }
        };
        let left = side(x0 - END_SLACK - 6..=x0 + 3, true);
        let right = side(x1 - 3..=x1 + END_SLACK + 6, false);
        if let (Some(l), Some(r)) = (left, right) {
            out.push(R { x0: l, y0: ty, x1: r + 1, y1: by });
        }
    }
    out
}

/// The smallest gap between content blocks along each axis.
const MIN_GAP: (i32, i32) = (16, 6);
/// How many levels of content blocks are kept below a panel or box.
const BLOCK_DEPTH: u32 = 3;

/// `r` shrunk to the pixels that hold content, or `None` when it holds none.
fn tighten(e: &Edges, r: R) -> Option<R> {
    let (x0, x1, y0, y1) = (r.x0 as usize, r.x1 as usize, r.y0 as usize, r.y1 as usize);
    let top = (y0..y1).find(|&y| e.row_inked(y, x0, x1))?;
    let bottom = (y0..y1).rev().find(|&y| e.row_inked(y, x0, x1))? + 1;
    let left = (x0..x1).find(|&x| e.col_inked(x, top, bottom))?;
    let right = (x0..x1).rev().find(|&x| e.col_inked(x, top, bottom))? + 1;
    Some(R { x0: left as i32, y0: top as i32, x1: right as i32, y1: bottom as i32 })
}

/// The content runs along one axis of `r` and the blank gaps between them.
fn spans(e: &Edges, r: R, horizontal: bool) -> Vec<(i32, i32)> {
    let mut out: Vec<(i32, i32)> = Vec::new();
    let (lo, hi) = if horizontal { (r.y0, r.y1) } else { (r.x0, r.x1) };
    let mut start: Option<i32> = None;
    for p in lo..hi {
        let inked = if horizontal {
            e.row_inked(p as usize, r.x0 as usize, r.x1 as usize)
        } else {
            e.col_inked(p as usize, r.y0 as usize, r.y1 as usize)
        };
        match (inked, start) {
            (true, None) => start = Some(p),
            (false, Some(s)) => {
                out.push((s, p));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push((s, hi));
    }
    out
}

/// Groups `spans` on gaps at least `threshold` wide.
fn group(spans: &[(i32, i32)], threshold: i32) -> Vec<(i32, i32)> {
    let mut out: Vec<(i32, i32)> = Vec::new();
    for &(a, b) in spans {
        match out.last_mut() {
            Some(last) if a - last.1 < threshold => last.1 = b,
            _ => out.push((a, b)),
        }
    }
    out
}

/// The gap that separates groups along one axis: the content's own rhythm. When the gaps are
/// alike (a list) every gap splits; when some are clearly larger (paragraphs over lines) only
/// those split here, and the smaller ones split on the next level.
fn split_gap(spans: &[(i32, i32)], min: i32) -> Option<i32> {
    let mut gaps: Vec<i32> = spans.windows(2).map(|w| w[1].0 - w[0].1).filter(|&g| g >= min).collect();
    if gaps.is_empty() {
        return None;
    }
    gaps.sort_unstable();
    let (small, large) = (gaps[0], gaps[gaps.len() - 1]);
    Some(if large * 2 >= small * 3 { (small + large + 1) / 2 } else { small })
}

/// Content blocks inside `r`: tightened to their content, split on the larger gaps first, then
/// again inside each block, alternating rows and columns.
fn blocks(e: &Edges, r: R, depth: u32, rows_first: bool, out: &mut Vec<R>) {
    if depth >= BLOCK_DEPTH {
        return;
    }
    let Some(r) = tighten(e, r) else { return };
    for horizontal in [rows_first, !rows_first] {
        let spans = spans(e, r, horizontal);
        let min = if horizontal { MIN_GAP.1 } else { MIN_GAP.0 };
        let Some(gap) = split_gap(&spans, min) else { continue };
        let parts = group(&spans, gap);
        if parts.len() < 2 {
            continue;
        }
        for (a, b) in parts {
            let part = if horizontal { R { y0: a, y1: b, ..r } } else { R { x0: a, x1: b, ..r } };
            let Some(part) = tighten(e, part) else { continue };
            out.push(part);
            blocks(e, part, depth + 1, !horizontal, out);
        }
        return;
    }
}

/// The regions inside `window` (a rect of `image`, in its pixels), in `image` coordinates:
/// panels and the boxes inside them, larger first. Empty when the window is too small or lies
/// outside the image.
pub fn regions(image: &Bitmap, window: PhysRect) -> Vec<PhysRect> {
    let x0 = window.x.max(0);
    let y0 = window.y.max(0);
    let x1 = (window.x + window.width as i32).min(image.width() as i32);
    let y1 = (window.y + window.height as i32).min(image.height() as i32);
    if x1 - x0 < 2 * MIN_SIZE || y1 - y0 < 2 * MIN_SIZE {
        return Vec::new();
    }
    let win = PhysRect { x: x0, y: y0, width: (x1 - x0) as u32, height: (y1 - y0) as u32 };
    let e = Edges::new(image, win);
    let whole = R { x0: 0, y0: 0, x1: e.w as i32, y1: e.h as i32 };
    let mut found = Vec::new();
    xy_cut(&e, whole, 0, &mut found);
    let panels = found.clone();
    let found_boxes = boxes(&e);
    // Content blocks inside the smallest panels (leaves) and inside each box, inset past its border.
    let leaves: Vec<R> = std::iter::once(whole)
        .chain(panels.iter().copied())
        .filter(|p| !panels.iter().any(|q| q != p && q.x0 >= p.x0 && q.y0 >= p.y0 && q.x1 <= p.x1 && q.y1 <= p.y1))
        .collect();
    for leaf in leaves {
        // Inset past the panel's own dividers: they ink every row and would hide every gap.
        let inner = R { x0: leaf.x0 + 3, y0: leaf.y0 + 3, x1: leaf.x1 - 3, y1: leaf.y1 - 3 };
        if inner.w() > 8 && inner.h() > 8 {
            blocks(&e, inner, 0, true, &mut found);
        }
    }
    for b in &found_boxes {
        let inner = R { x0: b.x0 + 4, y0: b.y0 + 4, x1: b.x1 - 4, y1: b.y1 - 4 };
        if inner.w() > 8 && inner.h() > 8 {
            blocks(&e, inner, 0, true, &mut found);
        }
    }
    found.extend(found_boxes);
    // Small, whole-window and duplicate regions go; larger regions come first.
    found.retain(|r| r.w() >= MIN_BOX.0 && r.h() >= MIN_BOX.1 && !r.near(whole, 2));
    found.sort_by_key(|r| std::cmp::Reverse(r.area()));
    let mut kept: Vec<R> = Vec::new();
    for r in found {
        if kept.iter().any(|k| k.iou(r) >= SAME_IOU || k.near(r, 2)) {
            continue;
        }
        kept.push(r);
        if kept.len() == MAX_REGIONS {
            break;
        }
    }
    kept.into_iter().map(|r| PhysRect { x: win.x + r.x0, y: win.y + r.y0, width: r.w() as u32, height: r.h() as u32 }).collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// A canvas filled with `bg`, with helpers to paint the parts of a mock UI.
    struct Canvas {
        w: u32,
        h: u32,
        px: Vec<u8>,
    }

    impl Canvas {
        fn new(w: u32, h: u32, bg: u8) -> Canvas {
            Canvas { w, h, px: [bg, bg, bg, 255].repeat((w * h) as usize) }
        }
        fn fill(&mut self, x: u32, y: u32, w: u32, h: u32, v: u8) {
            for yy in y..y + h {
                for xx in x..x + w {
                    let i = ((yy * self.w + xx) * 4) as usize;
                    self.px[i..i + 3].copy_from_slice(&[v, v, v]);
                }
            }
        }
        /// A 1 px outline.
        fn frame(&mut self, x: u32, y: u32, w: u32, h: u32, v: u8) {
            self.fill(x, y, w, 1, v);
            self.fill(x, y + h - 1, w, 1, v);
            self.fill(x, y, 1, h, v);
            self.fill(x + w - 1, y, 1, h, v);
        }
        /// "Text": short dark strokes that must not become regions.
        fn text(&mut self, x: u32, y: u32, words: u32, v: u8) {
            for i in 0..words {
                self.fill(x + i * 22, y, 14, 9, v);
            }
        }
        fn bitmap(self) -> Bitmap {
            Bitmap::new(self.w, self.h, PixelFormat::Rgba8, self.px.into()).unwrap()
        }
    }

    fn rect(x: i32, y: i32, w: u32, h: u32) -> PhysRect {
        PhysRect { x, y, width: w, height: h }
    }

    fn has_near(found: &[PhysRect], want: PhysRect, slack: i32) -> bool {
        found.iter().any(|r| {
            (r.x - want.x).abs() <= slack
                && (r.y - want.y).abs() <= slack
                && (r.width as i32 - want.width as i32).abs() <= slack
                && (r.height as i32 - want.height as i32).abs() <= slack
        })
    }

    /// A dark app: a sidebar, a header over the content, and a bordered input box at the bottom.
    fn mock_app() -> Bitmap {
        let mut c = Canvas::new(800, 500, 20);
        c.fill(0, 0, 200, 500, 32); // sidebar, a lighter panel
        c.fill(200, 0, 600, 48, 28); // header
        c.fill(200, 48, 600, 1, 60); // divider under the header
        c.text(220, 18, 6, 200);
        for row in 0..8 {
            c.text(230, 80 + row * 30, 12, 180); // chat text
            c.text(20, 30 + row * 40, 5, 170); // sidebar items
        }
        c.fill(240, 400, 520, 70, 34); // composer box fill
        c.frame(240, 400, 520, 70, 70); // and its border
        c.text(260, 420, 8, 150);
        c.bitmap()
    }

    #[test]
    fn panels_and_the_input_box_are_found() {
        let image = mock_app();
        let found = regions(&image, rect(0, 0, 800, 500));
        assert!(has_near(&found, rect(0, 0, 200, 500), 3), "sidebar: {found:?}");
        assert!(has_near(&found, rect(200, 0, 600, 48), 3), "header: {found:?}");
        assert!(has_near(&found, rect(240, 400, 520, 70), 3), "composer: {found:?}");
        assert!(found.len() < 40, "text must not become regions: {} found", found.len());
    }

    #[test]
    fn regions_are_in_image_coordinates_for_a_window_inside_the_screen() {
        let image = mock_app();
        let found = regions(&image, rect(200, 0, 600, 500));
        assert!(has_near(&found, rect(200, 0, 600, 48), 3), "{found:?}");
        assert!(found.iter().all(|r| r.x >= 200), "{found:?}");
    }

    #[test]
    fn a_flat_window_and_a_tiny_one_yield_nothing() {
        let flat = Canvas::new(300, 200, 40).bitmap();
        assert!(regions(&flat, rect(0, 0, 300, 200)).is_empty());
        assert!(regions(&flat, rect(0, 0, 30, 30)).is_empty());
        assert!(regions(&flat, rect(500, 500, 100, 100)).is_empty(), "outside the image");
    }
}
